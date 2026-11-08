export type ESCodeAgentBinaryKind = "native-binary";

export interface ESCodeAgentRuntimeDescriptor {
  binaryKind: ESCodeAgentBinaryKind;
  binaryEnvVar: string;
  bundledResourceDir: string;
  version: string;
  spawnArgs: string[];
  nativeConfigDir: string;
  nativeConfigFileName: string;
  missingBinaryMessage: string;
  resolveEntrySegments(platform: string): string[];
  /**
   * 官方插件仍由 Node 插件宿主（`__escode-plugin-host`）承载，Rust runtime 只负责 seed 与代理。
   * Node agent runtime 退场后安装包只携带这个极小入口（plugin-host.cjs），不再携带完整 agent bundle；
   * 它与 Rust 二进制同放 resources/glm。
   */
  pluginHostEntryFile: string;
  resolvePluginHostSegments(): string[];
  /**
   * Rust runtime（apps/escode-cli-rust）的原生二进制名，与插件宿主同放 glm 资源目录。
   * Node runtime 退场后它是唯一 agent runtime：打包态、开发态与远程部署都只用它。
   */
  rustBinaryName: string;
  resolveRustBinarySegments(platform: string): string[];
}

export function resolvePlatformBinaryName(binaryName: string, platform: string): string {
  return platform === "win32" ? `${binaryName}.exe` : binaryName;
}

export const ESCODE_AGENT_RUNTIME: ESCodeAgentRuntimeDescriptor = {
  binaryKind: "native-binary",
  binaryEnvVar: "GLM_BINARY_PATH",
  bundledResourceDir: "glm",
  version: "0.13.3",
  spawnArgs: ["app-server", "--stdio"],
  nativeConfigDir: ".escode/cli",
  nativeConfigFileName: "config.json",
  missingBinaryMessage:
    "[ESCode Agent] glm binary 未找到，请设置 ESCODE_AGENT_WORKDIR、GLM_BINARY_PATH 或先准备 GLM 运行时资源",
  resolveEntrySegments: (platform) => [resolvePlatformBinaryName("escode-agent", platform)],
  pluginHostEntryFile: "plugin-host.cjs",
  resolvePluginHostSegments() {
    return [this.pluginHostEntryFile];
  },
  rustBinaryName: "escode-cli-rust",
  resolveRustBinarySegments(platform) {
    return [resolvePlatformBinaryName(this.rustBinaryName, platform)];
  },
};

export function getESCodeAgentRuntime(): ESCodeAgentRuntimeDescriptor {
  return ESCODE_AGENT_RUNTIME;
}
