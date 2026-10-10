import { ESCODE_VERSION, type ESCodeEnv } from "@escode/shared";

declare const __ESCODE_CDN_BASE_URL__: string | undefined;
const DEFAULT_CDN_BASE_URL = "https://cdn-zcode.z.ai";

export interface ResolveRemoteCdnOptions {
  env?: ESCodeEnv;
  locale?: string;
  timeZone?: string;
  overrideBaseUrl?: string;
  version?: string;
  now?: Date;
}

function normalizeBaseUrl(value: string): string {
  const url = new URL(value);
  if (!["http:", "https:"].includes(url.protocol))
    throw new Error("CDN URL must use http or https");
  return value.replace(/\/+$/, "");
}

export function resolveRemoteCdnBaseUrls(options: ResolveRemoteCdnOptions = {}): string[] {
  const override = options.overrideBaseUrl?.trim();
  if (override) return [normalizeBaseUrl(override)];
  const baseUrl =
    process.env.ESCODE_CDN_BASE_URL?.trim() ||
    (typeof __ESCODE_CDN_BASE_URL__ === "undefined" ? "" : __ESCODE_CDN_BASE_URL__) ||
    DEFAULT_CDN_BASE_URL;
  return [
    `${normalizeBaseUrl(baseUrl)}/escode/electron/releases/${options.version ?? ESCODE_VERSION}`,
  ];
}
