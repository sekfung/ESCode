<<<<<<< HEAD:apps/escode-cli/packages/adapters/src/auth/coding-plan-api-key.ts
import type { HttpClientPort, HttpClientRunOptions, TraceContext } from "@escode/contracts";
import { resolveBigModelApiOrigin } from "@escode/shared";
=======
import { AsyncLocalStorage } from "node:async_hooks";
import {
  isHttpClientPortError,
  type HttpClientPort,
  type HttpClientRunOptions,
  type TraceContext,
} from "@zcode/contracts";
import {
  ProjectAccessTokenClient,
  ProjectAccessTokenTransientError,
  resolveBigModelApiOrigin,
  type ProjectAccessTokenMaterial,
} from "@zcode/shared";
import { ZaiBusinessTokenCache, type ZaiBusinessToken } from "./zai-business-token-cache.js";
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/adapters/src/auth/coding-plan-api-key.ts

const BUSINESS_AUTH_MAX_ATTEMPTS = 2;
const SINGLE_ATTEMPT = 1;
const PROJECT_TOKEN_REQUEST_TIMEOUT_MS = 15_000;
const ZAI_API_HOST = "https://api.z.ai";
<<<<<<< HEAD:apps/escode-cli/packages/adapters/src/auth/coding-plan-api-key.ts
const JSON_CONTENT_TYPE = "application/json";
const ESCODE_API_KEY_NAME = "escode-api-key";
const DEFAULT_ORG_NAME = "默认机构";
const DEFAULT_PROJECT_NAME = "默认项目";

=======
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/adapters/src/auth/coding-plan-api-key.ts
export type CodingPlanFamily = "bigmodel" | "zai";
export interface CodingPlanApiKeyResolverOptions {
  httpClient: HttpClientPort;
  trace?: TraceContext;
  locationStore?: {
    load(key: string): Promise<string | null>;
    save(key: string, value: string): Promise<void>;
    delete(key: string): Promise<void>;
  };
  observe?(event: { family: string; stage: "resolve"; code: string }): void;
}
export interface ResolveCodingPlanApiKeyInput {
  accessToken: string;
  family: CodingPlanFamily;
  accountIdentity?: string;
  rejectedProjectTokenFingerprint?: string;
  trace?: TraceContext;
}
export interface CodingPlanApiKeyResolver {
  resolve(input: ResolveCodingPlanApiKeyInput, options?: HttpClientRunOptions): Promise<string>;
  resolveMaterial(
    input: ResolveCodingPlanApiKeyInput,
    options?: HttpClientRunOptions,
  ): Promise<ProjectAccessTokenMaterial>;
  clear(): void;
}
export class CodingPlanApiKeyError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CodingPlanApiKeyError";
  }
}

/** CLI 进程内唯一换证入口；保留旧导出名兼容调用方，返回值已是短期 Token。 */
export function createCodingPlanApiKeyResolver(
  options: CodingPlanApiKeyResolverOptions,
): CodingPlanApiKeyResolver {
  const execution = new AsyncLocalStorage<{
    context?: HttpClientRunOptions["context"];
    trace?: TraceContext;
    business?: ZaiBusinessToken;
  }>();
  const businessTokens = new ZaiBusinessTokenCache(() =>
    options.observe?.({
      family: "zai",
      stage: "resolve",
      code: "project_token_business_login_refresh_deferred",
    }),
  );
<<<<<<< HEAD:apps/escode-cli/packages/adapters/src/auth/coding-plan-api-key.ts
  const location = pickOrgAndProject(customerInfo);
  if (!location) {
    throw new CodingPlanApiKeyError("Unable to resolve organization and project.");
  }

  const listUrl =
    `${input.host}/api/biz/v1/organization/${location.organizationId}` +
    `/projects/${location.projectId}/api_keys`;
  const keys =
    (await requestRemoteData<RemoteApiKeySummary[]>(
      input.httpClient,
      {
        headers: createBizAuthHeaders(input.authorization),
        method: "GET",
        trace: input.trace,
        url: listUrl,
      },
      runOptions,
    )) ?? [];
  const keyEntry =
    keys.find((item) => item.name === ESCODE_API_KEY_NAME) ??
    (await requestRemoteData<RemoteApiKeySummary>(
      input.httpClient,
      {
        body: new TextEncoder().encode(JSON.stringify({ name: ESCODE_API_KEY_NAME })),
        headers: createBizAuthHeaders(input.authorization),
        method: "POST",
        trace: input.trace,
        url: listUrl,
      },
      runOptions,
    ));
  const apiKey = keyEntry?.apiKey?.trim() ?? "";
  if (!apiKey) {
    throw new CodingPlanApiKeyError("API key response is missing apiKey.");
  }

  const secret = await requestRemoteData<RemoteApiKeySecret>(
    input.httpClient,
    {
      headers: createBizAuthHeaders(input.authorization),
      method: "GET",
      trace: input.trace,
      url: `${listUrl}/copy/${encodeURIComponent(apiKey)}`,
    },
    runOptions,
  );
  const secretKey = secret?.secretKey?.trim() ?? "";
  if (!secretKey) {
    if (input.requireSecretKey) {
      throw new CodingPlanApiKeyError("API key copy response is missing secretKey.");
=======
  const request = async (url: string, init: RequestInit) => {
    const response = await options.httpClient
      .request(
        {
          url,
          method: init.method === "POST" ? "POST" : "GET",
          headers: Object.fromEntries(new Headers(init.headers)),
          ...(typeof init.body === "string" ? { body: new TextEncoder().encode(init.body) } : {}),
          maxResponseBytes: 64 * 1024,
          timeoutMs: PROJECT_TOKEN_REQUEST_TIMEOUT_MS,
          trace: execution.getStore()?.trace ?? options.trace,
        },
        { context: execution.getStore()?.context },
      )
      .catch((error: unknown) => {
        if (
          isHttpClientPortError(error) &&
          (error.code === "network_error" || error.code === "timeout")
        )
          throw new ProjectAccessTokenTransientError();
        throw error;
      });
    if (response.status === 401 && new URL(url).pathname.startsWith("/api/biz/")) {
      const run = execution.getStore();
      if (
        run?.business &&
        new Headers(init.headers).get("Authorization") === `Bearer ${run.business.token}`
      )
        businessTokens.invalidate(run.business);
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/adapters/src/auth/coding-plan-api-key.ts
    }
    if (response.status === 404) return { code: 404 };
    if (response.status === 429 || (response.status >= 500 && response.status < 600))
      throw new ProjectAccessTokenTransientError();
    if (response.status < 200 || response.status >= 300)
      throw new CodingPlanApiKeyError("project_token_request_failed");
    return JSON.parse(new TextDecoder().decode(response.body)) as unknown;
  };
  const client = new ProjectAccessTokenClient({
    request,
    observe: options.observe,
    locationStore: options.locationStore,
  });
  let generation = 0;
  const resolveMaterial = async (
    input: ResolveCodingPlanApiKeyInput,
  ): Promise<ProjectAccessTokenMaterial> => {
    const oauth = input.accessToken.trim();
    if (!oauth) throw new CodingPlanApiKeyError("project_token_login_required");
    const capturedGeneration = generation;
    const maxAttempts = input.family === "zai" ? BUSINESS_AUTH_MAX_ATTEMPTS : SINGLE_ATTEMPT;
    for (let attempt = 0; attempt < maxAttempts; attempt++) {
      if (generation !== capturedGeneration)
        throw new CodingPlanApiKeyError("project_token_scope_invalidated");
      const business =
        input.family === "zai"
          ? await businessTokens
              .resolve(oauth, () =>
                request(`${ZAI_API_HOST}/api/auth/z/login`, {
                  method: "POST",
                  headers: { "Content-Type": "application/json" },
                  body: JSON.stringify({ token: oauth }),
                }),
              )
              .catch(() => {
                options.observe?.({
                  family: input.family,
                  stage: "resolve",
                  code: "project_token_login_failed",
                });
                throw new CodingPlanApiKeyError("project_token_login_failed");
              })
          : undefined;
      if (generation !== capturedGeneration)
        throw new CodingPlanApiKeyError("project_token_scope_invalidated");
      const loginToken = business?.token ?? oauth;
      const run = execution.getStore();
      if (run) run.business = business;
      try {
        const result = await client.resolve({
          origin: input.family === "zai" ? ZAI_API_HOST : resolveBigModelApiOrigin(process.env),
          family: input.family,
          loginToken,
          accountId: input.accountIdentity ?? input.family,
          personalProjectSelection: "default",
          rejectedProjectTokenFingerprint: input.rejectedProjectTokenFingerprint,
        });
        if (generation !== capturedGeneration)
          throw new CodingPlanApiKeyError("project_token_scope_invalidated");
        return result;
      } catch (error) {
        // 只恢复业务 JWT 的明确 HTTP 401；不把模型 401、403 或网络故障猜成过期。
        if (!business || !businessTokens.wasRejected(business)) throw error;
        businessTokens.invalidate(business);
        if (attempt + 1 === maxAttempts) throw error;
        options.observe?.({
          family: input.family,
          stage: "resolve",
          code: "project_token_business_login_refresh",
        });
      }
    }
    throw new CodingPlanApiKeyError("project_token_request_failed");
  };
  const resolveForCaller = (
    input: ResolveCodingPlanApiKeyInput,
    runOptions?: HttpClientRunOptions,
  ) =>
    waitForProjectToken(
      () =>
        execution.run(
          {
            // 单飞属于鉴权 owner，不继承会话 signal 或 context.abortSignal；只保留观测上下文。
            context: runOptions?.context
              ? { trace: runOptions.context.trace, logger: runOptions.context.logger }
              : undefined,
            trace: input.trace ?? runOptions?.context?.trace,
          },
          () => resolveMaterial(input),
        ),
      runOptions?.signal,
    );
  return {
    resolveMaterial: resolveForCaller,
    async resolve(input, runOptions) {
      const value = await resolveForCaller(input, runOptions);
      return value.token;
    },
    clear() {
      generation++;
      businessTokens.clear();
      client.clear();
    },
  };
}

/** 仅取消当前调用方的等待；底层 Promise 始终有拒绝处理器，不误伤共享换证。 */
async function waitForProjectToken<T>(run: () => Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return run();
  signal.throwIfAborted();
  const value = await new Promise<T>((resolve, reject) => {
    const cleanup = () => signal.removeEventListener("abort", onAbort);
    const onAbort = () => {
      cleanup();
      reject(signal.reason);
    };
    signal.addEventListener("abort", onAbort, { once: true });
    Promise.resolve()
      .then(() => {
        signal.throwIfAborted();
        return run();
      })
      .then(
        (result) => {
          cleanup();
          resolve(result);
        },
        (error: unknown) => {
          cleanup();
          reject(error);
        },
      );
  });
  signal.throwIfAborted();
  return value;
}
