import { createLocalServices, getAppConfigDir } from "@escode/services/node";
import {
  materializeBundledESCodeBuiltinProviderConfig,
  readBundledESCodeBuiltinProviderConfig,
} from "./bundledESCodeBuiltinProviderConfig.js";
import { createHttpServer } from "./http.js";

async function main(): Promise<void> {
  const escodeBuiltinProviderConfigFilePath = await materializeBundledESCodeBuiltinProviderConfig({
    environmentConfigRoot: getAppConfigDir(),
    content: readBundledESCodeBuiltinProviderConfig(),
  });
  const port = Number(process.env["PORT"]) || 3030;
  const host = process.env["ESCODE_SERVER_HOST"]?.trim() || process.env["HOST"]?.trim() || undefined;
  const staticRoot = process.env["ESCODE_WEB_STATIC_ROOT"]?.trim() || undefined;
  const authToken = process.env["ESCODE_SERVER_AUTH_TOKEN"]?.trim() || undefined;
  const services = createLocalServices({
    escodeBuiltinProviderConfigFilePath,
    providerProvisioningTargetEnabled: Boolean(authToken),
  });

  createHttpServer(services, port, {
    ...(host ? { host } : {}),
    ...(staticRoot ? { staticRoot, spaFallback: true } : {}),
    ...(authToken ? { authToken, authRequired: true } : {}),
  });
}

void main().catch((error: unknown) => {
  console.error("[escode-server:http] startup failed", error);
  process.exitCode = 1;
});
