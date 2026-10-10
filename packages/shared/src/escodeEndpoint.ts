import type { ESCodeEnv } from "./env.js";

export const DEFAULT_ESCODE_ENDPOINT_ORIGIN = "https://zcode.z.ai";
export const DEFAULT_BIGMODEL_API_ORIGIN = "https://bigmodel.cn";
export const DEFAULT_ZAI_OAUTH_ORIGIN = "https://chat.z.ai";
export const DEFAULT_ZAI_BUSINESS_BASE_URL = "https://api.z.ai";
export const DEFAULT_ZAI_OAUTH_CLIENT_ID = "client_P8X5CMWmlaRO9gyO-KSqtg";

// 构建仅注入公开链接；Node 调用方仍可显式传 env，避免读取另一进程的配置。
declare const __ESCODE_ENDPOINT_ENV__: Record<string, string | undefined> | undefined;
export function pickProductEndpointEnv(
  env: Record<string, string | undefined>,
): Record<string, string> {
  const keys = [
    "ESCODE_BASE_URL",
    "ESCODE_ENDPOINT_ORIGIN",
    "BIGMODEL_API_BASE_URL",
    "ZAI_OAUTH_ORIGIN",
    "ZAI_BUSINESS_BASE_URL",
    "ZAI_OAUTH_CLIENT_ID",
    "ZAI_OAUTH_APP_ID",
  ];
  return Object.fromEntries(
    keys.flatMap((key) => (env[key]?.trim() ? [[key, env[key]!.trim()]] : [])),
  );
}
export function readProductEndpointEnv(): Record<string, string | undefined> {
  return {
    ...(typeof __ESCODE_ENDPOINT_ENV__ === "undefined" ? {} : __ESCODE_ENDPOINT_ENV__),
    ...pickProductEndpointEnv(typeof process === "undefined" ? {} : process.env),
  };
}

export interface ESCodeEndpointUrls {
  origin: string;
  apiBaseUrl: string;
  webShareCallbackUrl: string;
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
}

export interface RuntimeBigModelApiEnv {
  [key: string]: string | undefined;
  ESCODE_ENV?: string;
  BIGMODEL_API_BASE_URL?: string;
}

export interface RuntimeZaiEndpointEnv {
  [key: string]: string | undefined;
  ESCODE_ENV?: string;
  ZAI_OAUTH_ORIGIN?: string;
  ZAI_BUSINESS_BASE_URL?: string;
  ZAI_OAUTH_CLIENT_ID?: string;
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

function isLoopbackHostname(hostname: string): boolean {
  return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "::1";
}

export function isTrustedCodingPlanWebviewOrigin(
  value: string | null | undefined,
  options?: {
    e2eStoreBridgeEnabled?: boolean;
  },
): boolean {
  if (!value) return false;
  try {
    const origin = normalizeESCodeEndpointOrigin(value);
    if (
      origin === DEFAULT_ESCODE_ENDPOINT_ORIGIN ||
      origin === resolveRuntimeESCodeEndpointOrigin()
    ) {
      return true;
    }
    const parsed = new URL(origin);
    return options?.e2eStoreBridgeEnabled === true && isLoopbackHostname(parsed.hostname);
  } catch {
    return false;
  }
}

export function resolveESCodeEndpointOrigin(options?: {
  env?: ESCodeEnv;
  envBaseOrigin?: string | null;
  overrideOrigin?: string | null;
}): string {
  const origin = options?.overrideOrigin?.trim() || options?.envBaseOrigin?.trim();
  return origin ? normalizeESCodeEndpointOrigin(origin) : DEFAULT_ESCODE_ENDPOINT_ORIGIN;
}

export function resolveRuntimeESCodeEnv(
  env: RuntimeESCodeEndpointEnv = readProductEndpointEnv(),
): ESCodeEnv {
  // 产品身份仅用于既有展示与安装标识，不参与地址解析。
  return env.ESCODE_ENV?.trim().toLowerCase() === "test" ? "test" : "production";
}

export function resolveRuntimeESCodeEndpointOrigin(
  env: RuntimeESCodeEndpointEnv = readProductEndpointEnv(),
  options?: { overrideOrigin?: string | null },
): string {
  return resolveESCodeEndpointOrigin({
    envBaseOrigin:
      readRuntimeEnvValue(env, "ESCODE_BASE_URL") ??
      readRuntimeEnvValue(env, "ESCODE_ENDPOINT_ORIGIN"),
    overrideOrigin: options?.overrideOrigin,
  });
}

export function buildRuntimeESCodeEndpointUrls(
  env: RuntimeESCodeEndpointEnv = readProductEndpointEnv(),
): ESCodeEndpointUrls {
  return buildESCodeEndpointUrls(resolveRuntimeESCodeEndpointOrigin(env));
}

export function buildRuntimeESCodeApiUrl(
  env: RuntimeESCodeEndpointEnv = readProductEndpointEnv(),
  path: string,
): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${resolveRuntimeESCodeEndpointOrigin(env)}${normalizedPath}`;
}

export function resolveBigModelApiOrigin(
  env: RuntimeBigModelApiEnv = readProductEndpointEnv(),
): string {
  return normalizeESCodeEndpointOrigin(
    readRuntimeEnvValue(env, "BIGMODEL_API_BASE_URL") ?? DEFAULT_BIGMODEL_API_ORIGIN,
  );
}

export function buildBigModelApiUrl(
  env: RuntimeBigModelApiEnv = readProductEndpointEnv(),
  path: string,
): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${resolveBigModelApiOrigin(env)}${normalizedPath}`;
}

export function buildBigModelCodingPlanPersonalManageUrl(
  env: RuntimeBigModelApiEnv = readProductEndpointEnv(),
): string {
  // 管理页与业务 API 共用显式 origin，避免把已登录账号带到另一个部署。
  return buildBigModelApiUrl(env, "/coding-plan/personal/overview");
}

export function buildBigModelCodingPlanTeamManageUrl(
  env: RuntimeBigModelApiEnv = readProductEndpointEnv(),
): string {
  return buildBigModelApiUrl(env, "/coding-plan/team/plans");
}

export function resolveZaiOAuthOrigin(
  env: RuntimeZaiEndpointEnv = readProductEndpointEnv(),
): string {
  return normalizeESCodeEndpointOrigin(
    readRuntimeEnvValue(env, "ZAI_OAUTH_ORIGIN") ?? DEFAULT_ZAI_OAUTH_ORIGIN,
  );
}

export function resolveZaiBusinessBaseUrl(
  env: RuntimeZaiEndpointEnv = readProductEndpointEnv(),
): string {
  return normalizeESCodeEndpointOrigin(
    readRuntimeEnvValue(env, "ZAI_BUSINESS_BASE_URL") ?? DEFAULT_ZAI_BUSINESS_BASE_URL,
  );
}

export function resolveZaiOAuthClientId(
  env: RuntimeZaiEndpointEnv = readProductEndpointEnv(),
): string {
  return (
    readRuntimeEnvValue(env, "ZAI_OAUTH_CLIENT_ID") ??
    readRuntimeEnvValue(env, "ZAI_OAUTH_APP_ID") ??
    DEFAULT_ZAI_OAUTH_CLIENT_ID
  );
}

export function buildZaiOAuthUrl(origin: string, path: string): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${normalizeESCodeEndpointOrigin(origin)}${normalizedPath}`;
}

export function buildRuntimeZaiOAuthUrl(
  env: RuntimeZaiEndpointEnv = readProductEndpointEnv(),
  path: string,
): string {
  return buildZaiOAuthUrl(resolveZaiOAuthOrigin(env), path);
}

export function buildRuntimeZaiBusinessUrl(
  env: RuntimeZaiEndpointEnv = readProductEndpointEnv(),
  path: string,
): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  return `${resolveZaiBusinessBaseUrl(env)}${normalizedPath}`;
}

export function resolveRuntimeProductEndpointConfig(
  env: RuntimeProductEndpointEnv = readProductEndpointEnv(),
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
    webShareCallbackUrl: `${normalizedOrigin}/cn/share/callback`,
    escodePlanOpenAiBaseUrl: `${normalizedOrigin}/api/v1/escode-plan`,
    escodePlanAnthropicBaseUrl: `${normalizedOrigin}/api/v1/escode-plan/anthropic`,
    escodePlanBillingCurrentUrl: `${normalizedOrigin}/api/v1/escode-plan/billing/current`,
    escodePlanBillingBalanceUrl: `${normalizedOrigin}/api/v1/escode-plan/billing/balance`,
  };
}

export function rewriteESCodeEndpointUrl(input: string | URL, endpointOrigin: string): string | URL {
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
