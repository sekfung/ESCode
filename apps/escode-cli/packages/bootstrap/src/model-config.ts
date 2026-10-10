import type {
  AiSdkModelExecutionConfig,
  AiSdkNetworkConfig,
  EnvRecord,
<<<<<<< HEAD:apps/escode-cli/packages/bootstrap/src/model-config.ts
} from "@escode/adapters/model";
=======
} from "@zcode/adapters/model";
import type { ProviderEndpointRoutingPort } from "@zcode/contracts";
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/bootstrap/src/model-config.ts
import {
  resolveRuntimeESCodeEnv,
  resolveRuntimeESCodeEndpointOrigin,
  ESCODE_APP_VERSION_ENV,
} from "@escode/shared";
import {
  createRuntimePlatformHeaders,
  normalizePrintableHeaderValue,
} from "./runtime-platform-headers.js";

export type ModelProviderSourceTitle = "cli" | "electron";

interface RuntimeExecutionConfigOptions {
  appVersion?: string;
  endpointRoutingPort?: ProviderEndpointRoutingPort;
  network?: AiSdkNetworkConfig;
  sourceTitle?: ModelProviderSourceTitle;
}

export function createRuntimeAiSdkModelExecutionConfig(
  env: EnvRecord = process.env,
  options: RuntimeExecutionConfigOptions = {},
): AiSdkModelExecutionConfig {
  const network = normalizeAiSdkNetworkConfig(options.network);
  return {
    defaultHeaders: buildCliESCodeSourceHeaders(env, options),
    env,
    ...(network ? { network } : {}),
    ...(options.endpointRoutingPort ? { endpointRoutingPort: options.endpointRoutingPort } : {}),
  };
}

function normalizeAiSdkNetworkConfig(
  network: AiSdkNetworkConfig | undefined,
): AiSdkNetworkConfig | undefined {
  if (!network?.caCertFile && !network?.httpProxy && !network?.noProxy) return undefined;
  return {
    ...(network.caCertFile ? { caCertFile: network.caCertFile } : {}),
    ...(network.httpProxy ? { httpProxy: network.httpProxy } : {}),
    ...(network.noProxy ? { noProxy: network.noProxy } : {}),
  };
}

<<<<<<< HEAD:apps/escode-cli/packages/bootstrap/src/model-config.ts
function buildCliESCodeSourceHeaders(
=======
export function buildCliZCodeSourceHeaders(
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/bootstrap/src/model-config.ts
  env: EnvRecord,
  options: Pick<RuntimeExecutionConfigOptions, "appVersion" | "sourceTitle"> = {},
): Record<string, string> {
  const sourceTitle = options.sourceTitle ?? detectDefaultProviderSourceTitle();
  const appVersion = resolveAppVersionForHeaders(env, options);
  const locale = normalizePrintableHeaderValue(Intl.DateTimeFormat().resolvedOptions().locale);
  const timezone = normalizePrintableHeaderValue(Intl.DateTimeFormat().resolvedOptions().timeZone);
  return {
    "HTTP-Referer": resolveRuntimeESCodeEndpointOrigin(env),
    "User-Agent": `ESCode/${appVersion ?? "unknown"}`,
    ...(appVersion ? { "X-ESCode-App-Version": appVersion } : {}),
    "X-Title": `Z Code@${sourceTitle}`,
    "X-Release-Channel": resolveRuntimeESCodeEnv(env),
    "X-Client-Language": locale ?? "unknown",
    "X-Client-Timezone": timezone ?? "unknown",
    "X-ESCode-Agent": "glm",
    ...createRuntimePlatformHeaders(),
  };
}

function resolveAppVersionForHeaders(
  env: EnvRecord,
  options: Pick<RuntimeExecutionConfigOptions, "appVersion">,
): string | undefined {
  return normalizePrintableHeaderValue(env[ESCODE_APP_VERSION_ENV] ?? options.appVersion);
}

function detectDefaultProviderSourceTitle(): ModelProviderSourceTitle {
  return process.argv.includes("app-server") || process.argv.includes("agent-server")
    ? "electron"
    : "cli";
}
