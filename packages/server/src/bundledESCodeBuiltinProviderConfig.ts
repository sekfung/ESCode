import { materializeESCodeBuiltinProviderConfig } from "@escode/services/node";

declare const __ESCODE_BUILTIN_PROVIDER_CONFIG_JSON__: string | undefined;

interface MaterializeBundledESCodeBuiltinProviderConfigOptions {
  readonly environmentConfigRoot: string;
  readonly content: string;
}

/** 返回构建时嵌入远端 Server 的 ESCode Built-in Provider Config。 */
export function readBundledESCodeBuiltinProviderConfig(): string {
  if (typeof __ESCODE_BUILTIN_PROVIDER_CONFIG_JSON__ !== "string") {
    throw new Error("当前构建未嵌入 ESCode Built-in Provider Config");
  }
  return __ESCODE_BUILTIN_PROVIDER_CONFIG_JSON__;
}

/**
 * 将 ESCode Built-in Config 原子物化到所属环境的固定资源副本。
 * 升级前退出旧进程；不保留按内容 hash 增长的历史文件。
 */
export async function materializeBundledESCodeBuiltinProviderConfig(
  options: MaterializeBundledESCodeBuiltinProviderConfigOptions,
): Promise<string> {
  return materializeESCodeBuiltinProviderConfig(options);
}
