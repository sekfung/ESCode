import { loadBuiltinProviderConfig } from "../../../../../scripts/builtin-provider-config.mjs";

export const SEA_ESCODE_BUILTIN_PROVIDER_CONFIG_ASSET_KEY = "escode-provider/escode-builtin.json";

export const collectSeaProviderConfigAssets = async ({ root, env = process.env }) => {
  const { sourcePath } = await loadBuiltinProviderConfig({ root, env });
  return {
    [SEA_ESCODE_BUILTIN_PROVIDER_CONFIG_ASSET_KEY]: sourcePath,
  };
};
