export const ESCODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV = "ESCODE_BUILTIN_PROVIDER_CONFIG_FILE";
export const ESCODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV =
  "ESCODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE";
export const ESCODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV = "ESCODE_PERSONAL_PROVIDER_CONFIG_FILE";
export const PERSONAL_PROVIDER_CONFIG_FILE_NAME = "provider_config.json";

export interface NodeProviderRuntimePaths {
  readonly escodeBuiltinFilePath: string;
  readonly personalFilePath: string;
}

export function createNodeProviderRuntimePathEnv(
  paths: NodeProviderRuntimePaths,
): Record<string, string> {
  return {
    [ESCODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV]: paths.escodeBuiltinFilePath,
    [ESCODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]: paths.personalFilePath,
  };
}

export function resolveNodeProviderRuntimePaths(
  env: Readonly<Record<string, string | undefined>>,
): NodeProviderRuntimePaths | null {
  const escodeBuiltinFilePath = env[ESCODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV]?.trim();
  const personalFilePath = env[ESCODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV]?.trim();
  if (!escodeBuiltinFilePath && !personalFilePath) return null;
  if (!escodeBuiltinFilePath || !personalFilePath) {
    throw new Error("ESCode Built-in 与 Personal Provider Config 路径必须同时提供");
  }
  return Object.freeze({ escodeBuiltinFilePath, personalFilePath });
}
