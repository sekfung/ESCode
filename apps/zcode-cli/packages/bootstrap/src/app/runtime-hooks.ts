// ============================================================
// 工作区的运行时 hooks 配置（与 createZCodeApp 启动同一口径）
// ============================================================
// Rust runtime 的工作流宿主据此为会话装配 hook 运行器（docs/specs/rust-hooks.md H1）：
// 用户 / env 层 hooks（createConfig）与插件 hook 来源（resolveStartupPlugins）按 mergeRuntimeHooks 合并。
// 工作区（项目）hooks 需要信任审核，属于 H2，这里不返回。

import type { HooksRuntimeConfig } from "@zcode/contracts";
import { createConfig, resolvePath } from "@zcode/adapters/config";
import type { Logger } from "@zcode/contracts";
import { StartupTimer, startupNow } from "../startup-logging.js";
import { getCliStorageRoot } from "./paths.js";
import { mergeRuntimeHooks } from "./runtime-config.js";
import { resolveStartupPlugins } from "./startup-marks.js";

const SILENT_LOGGER = {
  child: () => SILENT_LOGGER,
  debug: () => {},
  error: () => {},
  info: () => {},
  warn: () => {},
} as unknown as Logger;

export function resolveRuntimeHooks(input: {
  workingDirectory: string;
  env?: NodeJS.ProcessEnv;
}): HooksRuntimeConfig | undefined {
  const workingDirectory = resolvePath(input.workingDirectory);
  const configResult = createConfig({ env: input.env, workingDirectory });
  const cliStorageRoot = getCliStorageRoot(resolvePath(configResult.config.storage.dir));
  const plugins = resolveStartupPlugins({
    cliStorageRoot,
    configResult,
    env: input.env,
    logger: SILENT_LOGGER,
    options: {},
    startupTimer: new StartupTimer(SILENT_LOGGER, {}, startupNow()),
    workingDirectory,
  });
  const hooks = mergeRuntimeHooks(configResult.config.hooks, plugins.hooks);
  return hooks?.enabled ? hooks : undefined;
}
