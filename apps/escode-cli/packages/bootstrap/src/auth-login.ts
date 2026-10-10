import {
  createCodingPlanApiKeyResolver,
  createSharedESCodeCredentialStore,
  createCliOAuthClient,
  createCliOAuthPollToken,
  openUrlInBrowser,
  SHARED_ESCODE_CREDENTIAL_KEYS,
  type BrowserOpenResult,
  type SharedESCodeCredentialStore,
  type CliOAuthClient,
  type CliOAuthInitData,
  type CliOAuthPollData,
  type CliOAuthUser,
} from "@escode/adapters";
import { createConfig } from "@escode/adapters/config";
import { createNodeHttpClientAdapter } from "@escode/adapters/http";
import type { EnvRecord } from "@escode/adapters/model";
import { buildESCodeEndpointUrls, resolveRuntimeESCodeEndpointOrigin } from "@escode/shared";
import {
  NodeModelSelectionConfigRepository,
  NodePersonalProviderConfigRepository,
  PERSONAL_PROVIDER_CONFIG_FILE_NAME,
  ESCODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
} from "@escode/provider-node";
import { readLegacyCliPersonalProviderConfig } from "./app/legacy-cli-personal-provider-config-importer.js";
import { dirname, join } from "node:path";
import {
  createStandaloneAccountIdentityFromSecret,
  hasStandaloneCodingPlanAccess,
  readStandaloneCodingPlanProviders,
  resolveStandaloneCodingPlanProvider,
  standaloneAccountIdentityCredentialKey,
  standaloneAccountProviderCredentialKey,
} from "./app/standalone-account-provider-runtime.js";
import { throwIfAborted, waitWithAbort } from "./auth-login-abort.js";
import { setTimeout as delay } from "node:timers/promises";
import { pollUntilReady } from "./auth-login-polling.js";

const DEFAULT_LOGIN_TIMEOUT_MS = 5 * 60 * 1_000;

export type CodingPlanProviderId = "bigmodel" | "zai";

export interface LoginESCodeCliOptions {
  providerId?: CodingPlanProviderId;
  abortSignal?: AbortSignal;
  apiKeyResolver?: ReturnType<typeof createCodingPlanApiKeyResolver>;
  baseUrl?: string;
  credentialStore?: SharedESCodeCredentialStore;
  env?: EnvRecord;
  httpClient?: Parameters<typeof createCliOAuthClient>[0]["httpClient"];
  noBrowser?: boolean;
  now?: () => number;
  onAuthorizeUrl?: (data: CliOAuthInitData) => void | Promise<void>;
  onBrowserOpen?: (result: BrowserOpenResult) => void | Promise<void>;
  onPollStatus?: (data: CliOAuthPollData) => void | Promise<void>;
  openBrowser?: (url: string) => Promise<BrowserOpenResult>;
  pollToken?: string;
  sleep?: (ms: number) => Promise<void>;
  timeoutMs?: number;
  personalProviderConfigPath?: string;
}

export interface LoginESCodeCliResult {
  browser?: BrowserOpenResult;
  configPath: string;
  credentialsPath: string;
  model: string;
  providerId: CodingPlanProviderId;
  user: CliOAuthUser;
}

export type LoginBigmodelCodingPlanOptions = Omit<LoginESCodeCliOptions, "providerId">;
export type LoginBigmodelCodingPlanResult = LoginESCodeCliResult & { providerId: "bigmodel" };

export interface ConfigureCodingPlanApiKeyOptions {
  apiKey: string;
  credentialStore?: SharedESCodeCredentialStore;
  env?: EnvRecord;
  personalProviderConfigPath?: string;
  providerId: CodingPlanProviderId;
}

export interface ConfigureCodingPlanApiKeyResult {
  configPath: string;
  model: string;
  providerId: CodingPlanProviderId;
}

export interface LogoutESCodeCliOptions {
  credentialStore?: SharedESCodeCredentialStore;
  env?: EnvRecord;
}

export interface LogoutESCodeCliResult {
  credentialsPath: string;
}

export async function hasConfiguredStandaloneCodingPlan(
  options: {
    credentialStore?: SharedESCodeCredentialStore;
    env?: EnvRecord;
  } = {},
): Promise<boolean> {
  const credentialStore =
    options.credentialStore ?? createSharedESCodeCredentialStore({ env: options.env });
  return hasStandaloneCodingPlanAccess(credentialStore, options.env ?? process.env);
}

export class ESCodeCliLoginError extends Error {
  readonly code:
    | "auth_failed"
    | "auth_timeout"
    | "config_update_failed"
    | "credential_write_failed";

  constructor(
    code: ESCodeCliLoginError["code"],
    message: string,
    options: { cause?: unknown } = {},
  ) {
    super(message, options);
    this.name = "ESCodeCliLoginError";
    this.code = code;
  }
}

export async function loginESCodeCli(
  options: LoginESCodeCliOptions = {},
): Promise<LoginESCodeCliResult> {
  const env = options.env ?? process.env;
  const providerId = options.providerId ?? "zai";
  const now = options.now ?? Date.now;
  const timeoutMs = options.timeoutMs ?? DEFAULT_LOGIN_TIMEOUT_MS;
  const deadlineMs = now() + timeoutMs;
  const timeoutController = new AbortController();
  const signal = options.abortSignal
    ? AbortSignal.any([options.abortSignal, timeoutController.signal])
    : timeoutController.signal;
  const timeoutError = () =>
    new ESCodeCliLoginError("auth_timeout", "Authorization timed out. Please retry login.");
  let timer = setTimeout(() => timeoutController.abort(timeoutError()), timeoutMs);
  try {
    throwIfAborted(signal);
    const pollToken = options.pollToken ?? createCliOAuthPollToken();
    const credentialStore = options.credentialStore ?? createSharedESCodeCredentialStore({ env });
    const oauthClient = createOAuthClient(options, env);
    const initData = await waitWithAbort(oauthClient.init({ pollToken }, { signal }), signal);
    const remainingMs = Math.min(deadlineMs, initData.expires_at * 1_000) - now();
    if (remainingMs <= 0) throw timeoutError();
    clearTimeout(timer);
    timer = setTimeout(() => timeoutController.abort(timeoutError()), remainingMs);
    await options.onAuthorizeUrl?.(initData);
    throwIfAborted(signal);
    const browser = options.noBrowser
      ? undefined
      : await waitWithAbort(
          (options.openBrowser ?? openUrlInBrowser)(initData.authorize_url),
          signal,
        );
    if (browser) await options.onBrowserOpen?.(browser);
    const readyData = await pollUntilReady({
      abortSignal: signal,
      initData,
      now,
      oauthClient,
      onPollStatus: options.onPollStatus,
      pollToken,
      sleep: options.sleep ?? ((ms) => delay(ms, undefined, { signal })),
      timeoutMs: Math.max(0, deadlineMs - now()),
      createError: (code) =>
        code === "auth_timeout"
          ? timeoutError()
          : new ESCodeCliLoginError(code, "Authorization failed. Please retry login."),
    });
    const apiKey = await waitWithAbort(
      resolveCodingPlanApiKey({
        accessToken: readyData.accessToken,
        env,
        httpClient: options.httpClient,
        family: providerId,
        resolver: options.apiKeyResolver,
        signal,
      }),
      signal,
    );
    // A cancelled/expired attempt must not persist a late ready response or API key.
    throwIfAborted(signal);
    try {
      if (providerId === "zai") {
        await credentialStore.saveZaiLoginCredentials({
          accessToken: readyData.accessToken,
          jwtToken: readyData.token,
          user: readyData.user,
        });
      } else {
        await credentialStore.saveMany({
          [SHARED_ESCODE_CREDENTIAL_KEYS.activeProvider]: providerId,
          [SHARED_ESCODE_CREDENTIAL_KEYS.escodeJwtToken]: readyData.token,
          [SHARED_ESCODE_CREDENTIAL_KEYS.bigmodelAccessToken]: readyData.accessToken,
          ...(readyData.refreshToken
            ? { [SHARED_ESCODE_CREDENTIAL_KEYS.bigmodelRefreshToken]: readyData.refreshToken }
            : {}),
          [SHARED_ESCODE_CREDENTIAL_KEYS.bigmodelUserInfo]: JSON.stringify({
            id: readyData.user.user_id,
            username: readyData.user.name || readyData.user.email || readyData.user.user_id,
            displayName: readyData.user.name || readyData.user.email || readyData.user.user_id,
            rawProfile: readyData.user,
          }),
        });
      }
    } catch (error) {
      throw new ESCodeCliLoginError(
        "credential_write_failed",
        "Login succeeded but writing credentials failed.",
        { cause: error },
      );
    }
    throwIfAborted(signal);
    let configPatch: StandaloneCodingPlanPersistenceResult;
    try {
      configPatch = await persistStandaloneCodingPlanConnection({
        accountIdentity: readyData.user.user_id,
        apiKey,
        credentialStore,
        env,
        personalProviderConfigPath: options.personalProviderConfigPath,
        providerId,
      });
    } catch (error) {
      throw new ESCodeCliLoginError(
        "config_update_failed",
        "Login succeeded but updating ESCode config failed.",
        { cause: error },
      );
    }
    return {
      ...(browser ? { browser } : {}),
      configPath: configPatch.path,
      credentialsPath: credentialStore.filePath,
      model: configPatch.mainModel,
      providerId,
      user: readyData.user,
    };
  } finally {
    clearTimeout(timer);
  }
}

export async function loginBigmodelCodingPlan(
  options: LoginBigmodelCodingPlanOptions = {},
): Promise<LoginBigmodelCodingPlanResult> {
  return {
    ...(await loginESCodeCli({ ...options, providerId: "bigmodel" })),
    providerId: "bigmodel",
  };
}

export async function configureCodingPlanApiKey(
  options: ConfigureCodingPlanApiKeyOptions,
): Promise<ConfigureCodingPlanApiKeyResult> {
  const apiKey = options.apiKey.trim();
  if (!apiKey) {
    throw new ESCodeCliLoginError("config_update_failed", "API key must not be empty.");
  }
  const credentialStore =
    options.credentialStore ?? createSharedESCodeCredentialStore({ env: options.env });
  const configPatch = await persistStandaloneCodingPlanConnection({
    accountIdentity: createStandaloneAccountIdentityFromSecret(apiKey),
    apiKey,
    credentialStore,
    env: options.env ?? process.env,
    personalProviderConfigPath: options.personalProviderConfigPath,
    providerId: options.providerId,
  });
  return {
    configPath: configPatch.path,
    model: configPatch.mainModel,
    providerId: options.providerId,
  };
}

export async function logoutESCodeCli(
  options: LogoutESCodeCliOptions = {},
): Promise<LogoutESCodeCliResult> {
  const credentialStore =
    options.credentialStore ?? createSharedESCodeCredentialStore({ env: options.env });
  const providerIds = (await readStandaloneCodingPlanProviders(options.env ?? process.env)).map(
    ({ providerId }) => providerId,
  );
  const identityKeys = providerIds.map(standaloneAccountIdentityCredentialKey);
  const identities = await credentialStore.loadMany(identityKeys);
  const dynamicApiKeyKeys = providerIds.flatMap((providerId) => {
    const identity = identities[standaloneAccountIdentityCredentialKey(providerId)]?.trim();
    return identity
      ? [
          standaloneAccountProviderCredentialKey({
            providerId,
            accountIdentity: identity,
          }),
        ]
      : [];
  });
  const keys = [
    ...Object.values(SHARED_ESCODE_CREDENTIAL_KEYS),
    ...identityKeys,
    ...dynamicApiKeyKeys,
  ];
  const current = await credentialStore.loadMany(keys);
  await credentialStore.deleteIfValues(
    Object.fromEntries(
      Object.entries(current).flatMap(([key, value]) => (value === null ? [] : [[key, value]])),
    ),
  );
  return {
    credentialsPath: credentialStore.filePath,
  };
}

interface StandaloneCodingPlanPersistenceResult {
  readonly mainModel: string;
  readonly path: string;
}

async function persistStandaloneCodingPlanConnection(input: {
  readonly accountIdentity: string;
  readonly apiKey: string;
  readonly credentialStore: SharedESCodeCredentialStore;
  readonly env: EnvRecord;
  readonly personalProviderConfigPath?: string;
  readonly providerId: CodingPlanProviderId;
}): Promise<StandaloneCodingPlanPersistenceResult> {
  const configuredProvider = await resolveStandaloneCodingPlanProvider(input.providerId, input.env);
  const providerId = configuredProvider.providerId;
  const modelId = configuredProvider.modelId;
  const credentialKey = standaloneAccountProviderCredentialKey({
    providerId,
    accountIdentity: input.accountIdentity,
  });
  await input.credentialStore.saveMany({
    [standaloneAccountIdentityCredentialKey(providerId)]: input.accountIdentity,
    [credentialKey]: input.apiKey,
  });
  const path =
    input.personalProviderConfigPath ??
    input.env[ESCODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]?.trim() ??
    join(dirname(input.credentialStore.filePath), PERSONAL_PROVIDER_CONFIG_FILE_NAME);
  // 登录与运行时共享文件和事务；首次写入仍先保留旧用户 Provider，不能仅写默认值。
  const personalRepository = new NodePersonalProviderConfigRepository({
    filePath: path,
    importLegacy: () => readLegacyCliPersonalProviderConfig({}),
    pollingIntervalMs: false,
  });
  const repository = new NodeModelSelectionConfigRepository({ personalRepository });
  try {
    await repository.saveConfiguredDefault({ providerId, modelId });
  } finally {
    repository.dispose();
    personalRepository.dispose();
  }
  return {
    mainModel: `${providerId}/${modelId}`,
    path,
  };
}

function createOAuthClient(options: LoginESCodeCliOptions, env: EnvRecord): CliOAuthClient {
  return createCliOAuthClient({
    baseUrl:
      options.baseUrl ?? buildESCodeEndpointUrls(resolveCliESCodeEndpointOrigin(env)).apiBaseUrl,
    providerId: options.providerId ?? "zai",
    httpClient: options.httpClient ?? createDefaultHttpClient(env),
  });
}

function resolveCliESCodeEndpointOrigin(env: EnvRecord): string {
  return resolveRuntimeESCodeEndpointOrigin(env);
}

function createDefaultHttpClient(env: EnvRecord) {
  const config = createConfig({ env });
  return createNodeHttpClientAdapter({
    env,
    proxyUrl: config.config.network.httpProxy,
    noProxy: config.config.network.noProxy,
    caCertFile: config.config.network.caCertFile,
    timeoutMs: config.config.network.timeout,
  });
}

async function resolveCodingPlanApiKey(input: {
  accessToken: string;
  env: EnvRecord;
  httpClient?: Parameters<typeof createCodingPlanApiKeyResolver>[0]["httpClient"];
  family: CodingPlanProviderId;
  resolver?: ReturnType<typeof createCodingPlanApiKeyResolver>;
  signal?: AbortSignal;
}): Promise<string> {
  const resolver =
    input.resolver ??
    createCodingPlanApiKeyResolver({
      httpClient: input.httpClient ?? createDefaultHttpClient(input.env),
    });
  return resolver.resolve(
    {
      accessToken: input.accessToken,
      family: input.family,
    },
    { signal: input.signal },
  );
}
