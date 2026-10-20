import type { ESCodeEnv } from "./env.js";

export const DEFAULT_ESCODE_ENDPOINT_ORIGIN = "https://zcode.z.ai";
export const TEST_ESCODE_ENDPOINT_ORIGIN = "https://zcode.z.ai";
export const DEFAULT_BIGMODEL_API_ORIGIN = "https://bigmodel.cn";
export const TEST_BIGMODEL_API_ORIGIN = "https://bigmodel.cn";
export const DEFAULT_ZAI_OAUTH_ORIGIN = "https://chat.z.ai";
export const TEST_ZAI_OAUTH_ORIGIN = "https://chat.z.ai";
export const DEFAULT_ZAI_BUSINESS_BASE_URL = "https://api.z.ai";
export const TEST_ZAI_BUSINESS_BASE_URL = "https://api.z.ai";
export const DEFAULT_ZAI_OAUTH_CLIENT_ID = "client_P8X5CMWmlaRO9gyO-KSqtg";
export const TEST_ZAI_OAUTH_CLIENT_ID = "client_P8X5CMWmlaRO9gyO-KSqtg";

export interface ESCodeEndpointUrls {
  origin: string;
  apiBaseUrl: string;
  escodePlanOpenAiBaseUrl: string;
  escodePlanAnthropicBaseUrl: string;
  escodePlanBillingCurrentUrl: string;
  escodePlanBillingBalanceUrl: string;
}

export interface RuntimeESCodeEndpointEnv {
  [key: string]: string | undefined;
  ESCODE_ENV?: string;
  ESCODE_BASE_URL?: string;
  ESCODE_ENDPOINT_ORIGIN?: string;
  ESCODE_TEST_BASE_URL?: string;
  ESCODE_PRODUCTION_BASE_URL?: string;
}

export interface RuntimeBigModelApiEnv {
  [key: string]: string | undefined;
  ESCODE_ENV?: string;
  BIGMODEL_API_BASE_URL?: string;
  BIGMODEL_TEST_API_BASE_URL?: string;
  BIGMODEL_PRODUCTION_API_BASE_URL?: string;
}

export interface RuntimeZaiEndpointEnv {
  [key: string]: string | undefined;
  ESCODE_ENV?: string;
  ZAI_OAUTH_ORIGIN?: string;
  ZAI_TEST_OAUTH_ORIGIN?: string;
  ZAI_PRODUCTION_OAUTH_ORIGIN?: string;
  ZAI_BUSINESS_BASE_URL?: string;
  ZAI_TEST_BUSINESS_BASE_URL?: string;
  ZAI_PRODUCTION_BUSINESS_BASE_URL?: string;
  ZAI_OAUTH_CLIENT_ID?: string;
  ZAI_TEST_OAUTH_CLIENT_ID?: string;
  ZAI_PRODUCTION_OAUTH_CLIENT_ID?: string;
  ZAI_OAUTH_APP_ID?: string;
}

export interface RuntimeProductEndpointEnv
  extends RuntimeESCodeEndpointEnv, RuntimeBigModelApiEnv, RuntimeZaiEndpointEnv {}

export interface RuntimeProductEndpointConfig {
  escodeEnv: ESCodeEnv;
  escodeEndpointOrigin: string;
  escodeEndpointUrls: ESCodeEndpointUrls;
  zaiOAuthOrigin: string;
  zaiBusinessBaseUrl: string;
  zaiOAuthClientId: string;
  bigModelApiOrigin: string;
}

function readRuntimeEnvValue(
  env: Record<string, string | undefined>,
  key: string,
): string | undefined {
  const value = env[key]?.trim();
  return value ? value : undefined;
}

function readRuntimeESCodeEnvScopedValue(
  env: Record<string, string | undefined>,
  escodeEnv: ESCodeEnv,
  testKey: string,
  productionKey: string,
): string | undefined {
  return readRuntimeEnvValue(env, escodeEnv === "production" ? productionKey : testKey);
}

export function normalizeESCodeEndpointOrigin(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) {
    throw new Error("ESCode endpoint origin is empty");
  }

  const parsed = new URL(trimmed);
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
    throw new Error("ESCode endpoint origin must use http or https");
  }
  return parsed.origin;
}

export function resolveESCodeEndpointOrigin(options?: {
  env?: ESCodeEnv;
  envBaseOrigin?: string | null;
  overrideOrigin?: string | null;
}): string {
  const env = options?.env ?? "test";
  const envBaseOrigin = options?.envBaseOrigin?.trim();
  if (env === "production") {
    return envBaseOrigin
      ? normalizeESCodeEndpointOrigin(envBaseOrigin)
      : DEFAULT_ESCODE_ENDPOINT_ORIGIN;
  }

  const overrideOrigin = options?.overrideOrigin?.trim();
  if (overrideOrigin) {
    return normalizeESCodeEndpointOrigin(overrideOrigin);
  }

  return envBaseOrigin ? normalizeESCodeEndpointOrigin(envBaseOrigin) : TEST_ESCODE_ENDPOINT_ORIGIN;
}

export function resolveRuntimeESCodeEnv(env: RuntimeESCodeEndpointEnv = {}): ESCodeEnv {
  // 独立 CLI / 服务测试未注入 ESCODE_ENV 时默认生产域；桌面测试包会显式下发 ESCODE_ENV=test。
  return env.ESCODE_ENV?.trim().toLowerCase() === "test" ? "test" : "production";
}

export function resolveRuntimeESCodeEndpointOrigin(
  env: RuntimeESCodeEndpointEnv = {},
  options?: { overrideOrigin?: string | null },
): string {
  const escodeEnv = resolveRuntimeESCodeEnv(env);
  const scopedBaseOrigin =
    escodeEnv === "production"
      ? readRuntimeEnvValue(env, "ESCODE_PRODUCTION_BASE_URL")
      : readRuntimeEnvValue(env, "ESCODE_TEST_BASE_URL");

  return resolveESCodeEndpointOrigin({
    env: escodeEnv,
    envBaseOrigin:
      readRuntimeEnvValue(env, "ESCODE_BASE_URL") ??
      readRuntimeEnvValue(env, "ESCODE_ENDPOINT_ORIGIN") ??
      scopedBaseOrigin,
    overrideOrigin: options?.overrideOrigin,
  });
}

export function buildRuntimeESCodeEndpointUrls(
  env: RuntimeESCodeEndpointEnv = {},
): ESCodeEndpointUrls {
  return buildESCodeEndpointUrls(resolveRuntimeESCodeEndpointOrigin(env));
}

export function buildRuntimeESCodeApiUrl(env: RuntimeESCodeEndpointEnv = {}, path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${resolveRuntimeESCodeEndpointOrigin(env)}${normalizedPath}`;
}

export function resolveBigModelApiOrigin(env: RuntimeBigModelApiEnv = {}): string {
  const escodeEnv = resolveRuntimeESCodeEnv(env);
  const scopedBaseOrigin =
    escodeEnv === "production"
      ? readRuntimeEnvValue(env, "BIGMODEL_PRODUCTION_API_BASE_URL")
      : readRuntimeEnvValue(env, "BIGMODEL_TEST_API_BASE_URL");
  const fallbackOrigin =
    escodeEnv === "production" ? DEFAULT_BIGMODEL_API_ORIGIN : TEST_BIGMODEL_API_ORIGIN;

  return normalizeESCodeEndpointOrigin(
    readRuntimeEnvValue(env, "BIGMODEL_API_BASE_URL") ?? scopedBaseOrigin ?? fallbackOrigin,
  );
}

export function buildBigModelApiUrl(env: RuntimeBigModelApiEnv = {}, path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${resolveBigModelApiOrigin(env)}${normalizedPath}`;
}

export function resolveZaiOAuthOrigin(env: RuntimeZaiEndpointEnv = {}): string {
  const escodeEnv = resolveRuntimeESCodeEnv(env);
  const scopedOrigin = readRuntimeESCodeEnvScopedValue(
    env,
    escodeEnv,
    "ZAI_TEST_OAUTH_ORIGIN",
    "ZAI_PRODUCTION_OAUTH_ORIGIN",
  );
  const fallbackOrigin =
    escodeEnv === "production" ? DEFAULT_ZAI_OAUTH_ORIGIN : TEST_ZAI_OAUTH_ORIGIN;

  return normalizeESCodeEndpointOrigin(
    readRuntimeEnvValue(env, "ZAI_OAUTH_ORIGIN") ?? scopedOrigin ?? fallbackOrigin,
  );
}

export function resolveZaiBusinessBaseUrl(env: RuntimeZaiEndpointEnv = {}): string {
  const escodeEnv = resolveRuntimeESCodeEnv(env);
  const scopedOrigin = readRuntimeESCodeEnvScopedValue(
    env,
    escodeEnv,
    "ZAI_TEST_BUSINESS_BASE_URL",
    "ZAI_PRODUCTION_BUSINESS_BASE_URL",
  );
  const fallbackOrigin =
    escodeEnv === "production" ? DEFAULT_ZAI_BUSINESS_BASE_URL : TEST_ZAI_BUSINESS_BASE_URL;

  return normalizeESCodeEndpointOrigin(
    readRuntimeEnvValue(env, "ZAI_BUSINESS_BASE_URL") ?? scopedOrigin ?? fallbackOrigin,
  );
}

export function resolveZaiOAuthClientId(env: RuntimeZaiEndpointEnv = {}): string {
  const escodeEnv = resolveRuntimeESCodeEnv(env);
  const scopedClientId = readRuntimeESCodeEnvScopedValue(
    env,
    escodeEnv,
    "ZAI_TEST_OAUTH_CLIENT_ID",
    "ZAI_PRODUCTION_OAUTH_CLIENT_ID",
  );
  const fallbackClientId =
    escodeEnv === "production" ? DEFAULT_ZAI_OAUTH_CLIENT_ID : TEST_ZAI_OAUTH_CLIENT_ID;

  return (
    readRuntimeEnvValue(env, "ZAI_OAUTH_CLIENT_ID") ??
    scopedClientId ??
    readRuntimeEnvValue(env, "ZAI_OAUTH_APP_ID") ??
    fallbackClientId
  );
}

export function buildZaiOAuthUrl(origin: string, path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${normalizeESCodeEndpointOrigin(origin)}${normalizedPath}`;
}

export function buildRuntimeZaiOAuthUrl(env: RuntimeZaiEndpointEnv = {}, path: string): string {
  return buildZaiOAuthUrl(resolveZaiOAuthOrigin(env), path);
}

export function buildRuntimeZaiBusinessUrl(env: RuntimeZaiEndpointEnv = {}, path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${resolveZaiBusinessBaseUrl(env)}${normalizedPath}`;
}

export function resolveRuntimeProductEndpointConfig(
  env: RuntimeProductEndpointEnv = {},
): RuntimeProductEndpointConfig {
  const escodeEnv = resolveRuntimeESCodeEnv(env);
  const escodeEndpointOrigin = resolveRuntimeESCodeEndpointOrigin(env);

  return {
    escodeEnv,
    escodeEndpointOrigin,
    escodeEndpointUrls: buildESCodeEndpointUrls(escodeEndpointOrigin),
    zaiOAuthOrigin: resolveZaiOAuthOrigin(env),
    zaiBusinessBaseUrl: resolveZaiBusinessBaseUrl(env),
    zaiOAuthClientId: resolveZaiOAuthClientId(env),
    bigModelApiOrigin: resolveBigModelApiOrigin(env),
  };
}

export function buildESCodeEndpointUrls(origin: string): ESCodeEndpointUrls {
  const normalizedOrigin = normalizeESCodeEndpointOrigin(origin);

  return {
    origin: normalizedOrigin,
    apiBaseUrl: `${normalizedOrigin}/api/v1`,
    escodePlanOpenAiBaseUrl: `${normalizedOrigin}/api/v1/zcode-plan`,
    escodePlanAnthropicBaseUrl: `${normalizedOrigin}/api/v1/zcode-plan/anthropic`,
    escodePlanBillingCurrentUrl: `${normalizedOrigin}/api/v1/zcode-plan/billing/current`,
    escodePlanBillingBalanceUrl: `${normalizedOrigin}/api/v1/zcode-plan/billing/balance`,
  };
}

export function rewriteESCodeEndpointUrl(
  input: string | URL,
  endpointOrigin: string,
): string | URL {
  const originalUrl = typeof input === "string" ? input : input.toString();
  let parsed: URL;
  try {
    parsed = new URL(originalUrl);
  } catch {
    return input;
  }
  const sourceOrigin = DEFAULT_ESCODE_ENDPOINT_ORIGIN;
  if (parsed.origin !== sourceOrigin) {
    return input;
  }

  const targetOrigin = normalizeESCodeEndpointOrigin(endpointOrigin);
  if (targetOrigin === sourceOrigin) {
    return input;
  }

  const target = new URL(targetOrigin);
  target.pathname = parsed.pathname;
  target.search = parsed.search;
  target.hash = parsed.hash;
  return target.toString();
}
