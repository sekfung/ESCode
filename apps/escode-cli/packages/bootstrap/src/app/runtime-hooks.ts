// ============================================================
// 工作区的运行时 hooks 配置（与 createESCodeApp 启动同一口径）
// ============================================================
// Rust runtime 的工作流宿主据此为会话装配 hook 运行器（docs/specs/rust-hooks.md H1）：
// 用户 / env 层 hooks（createConfig）与插件 hook 来源（resolveStartupPlugins）按 mergeRuntimeHooks 合并。
// 工作区（项目）hooks 需要信任审核（H2）：由 `resolveRuntimeHookContext` 一并返回配置来源，宿主据此装配
// `createWorkspaceHookRuntimeSecurity`（与 createESCodeApp 同一口径）。

import type { HooksRuntimeConfig } from "@escode/contracts";
import { createConfig, resolvePath } from "@escode/adapters/config";
import type { Logger } from "@escode/contracts";
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
  const hooks = resolveRuntimeHookContext(input).hooks;
  return hooks?.enabled ? hooks : undefined;
}

export function resolveRuntimeHookContext(input: {
  workingDirectory: string;
  env?: NodeJS.ProcessEnv;
}): { hooks: HooksRuntimeConfig | undefined; configResult: ReturnType<typeof createConfig> } {
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
  return { hooks: mergeRuntimeHooks(configResult.config.hooks, plugins.hooks), configResult };
}
