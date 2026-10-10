import { DEFAULT_ESCODE_ENDPOINT_ORIGIN } from "./escodeEndpoint.js";

export const ESCODE_SOURCE_HEADERS = {
  "User-Agent": "ESCode/unknown",
  "HTTP-Referer": DEFAULT_ESCODE_ENDPOINT_ORIGIN,
  "X-Title": "Z Code@electron",
} as const;

export interface BuildESCodeSourceHeadersFromContextOptions {
  appVersion?: string;
  arch?: string;
  clientLanguage?: string;
  clientTimezone?: string;
  deviceMid?: string;
  endpointOrigin?: string;
  osVersion?: string;
  platform?: string;
  releaseChannel?: string;
  sourceTitle?: string;
}

export function normalizeESCodeSourceHeaderValue(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  if (!trimmed || !/^[\x20-\x7e]+$/.test(trimmed)) {
    return undefined;
  }
  return trimmed;
}

export function buildESCodeSourceHeadersFromContext(
  options: BuildESCodeSourceHeadersFromContextOptions = {},
): Record<string, string> {
  const appVersion = normalizeESCodeSourceHeaderValue(options.appVersion);
  const arch = normalizeESCodeSourceHeaderValue(options.arch);
  const clientLanguage = normalizeESCodeSourceHeaderValue(options.clientLanguage) ?? "unknown";
  const clientTimezone = normalizeESCodeSourceHeaderValue(options.clientTimezone) ?? "unknown";
  const deviceMid = normalizeESCodeSourceHeaderValue(options.deviceMid);
  const endpointOrigin =
    normalizeESCodeSourceHeaderValue(options.endpointOrigin) ?? DEFAULT_ESCODE_ENDPOINT_ORIGIN;
  const osVersion = normalizeESCodeSourceHeaderValue(options.osVersion);
  const platform = normalizeESCodeSourceHeaderValue(options.platform);
  const releaseChannel = normalizeESCodeSourceHeaderValue(options.releaseChannel);
  const sourceTitle = normalizeESCodeSourceHeaderValue(options.sourceTitle) ?? "electron";

  return {
    ...ESCODE_SOURCE_HEADERS,
    "HTTP-Referer": endpointOrigin,
    "User-Agent": `ESCode/${appVersion ?? "unknown"}`,
    ...(appVersion ? { "X-ESCode-App-Version": appVersion } : {}),
    "X-Title": `Z Code@${sourceTitle}`,
    ...(platform && arch ? { "X-Platform": `${platform}-${arch}` } : {}),
    ...(releaseChannel ? { "X-Release-Channel": releaseChannel } : {}),
    "X-Client-Language": clientLanguage,
    "X-Client-Timezone": clientTimezone,
    ...(platform ? { "X-Os-Category": normalizeOsCategory(platform) } : {}),
    ...(osVersion ? { "X-Os-Version": osVersion } : {}),
    ...(deviceMid ? { "X-Device-Mid": deviceMid } : {}),
  };
}

function normalizeOsCategory(platform: string): string {
  switch (platform) {
    case "darwin":
      return "macos";
    case "win32":
      return "windows";
    default:
      return "linux";
  }
}
