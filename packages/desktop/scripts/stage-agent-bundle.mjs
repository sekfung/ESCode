// Agent bundle 的暂存动作：把 apps/escode-cli/packages/cli/dist/plugin-host.cjs
// 放进 bundled-agents/<平台>/glm。Node agent runtime 退场后不再暂存 zcode.cjs 与 meta，
// 只保留官方插件宿主（docs/specs/rust-packaging.md）。
//
// dev 与打包**必须**用同一份暂存实现。
// 只有打包链（prepare-agent-node-bundle.mjs）会暂存是不够的，dev 链
// （scripts/build-desktop-agent-cli.mjs）不会；两边共用这一份，避免 dev 与打包各自漂移。
import { copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { resolve } from "node:path";

export const PLUGIN_HOST_BUNDLE_SOURCE_RELATIVE =
  "apps/escode-cli/packages/cli/dist/plugin-host.cjs";

export function resolveAgentBundlePaths({ repoRoot, platformKey }) {
  const glmDir = resolve(repoRoot, "packages", "desktop", "bundled-agents", platformKey, "glm");
  return {
    pluginHostBundlePath: resolve(repoRoot, PLUGIN_HOST_BUNDLE_SOURCE_RELATIVE),
    glmDir,
    stagedPluginHostPath: resolve(glmDir, "plugin-host.cjs"),
  };
}

/**
 * 干净重建 glm 目录再拷贝。清空是刻意的：electron-builder 整目录拷贝
 * bundled-agents/<平台>/glm → resources/glm，本地工作树里上一次构建残留的原生二进制
 * （escode-agent / escode-acp 等）、旧 zcode.cjs 与 meta 会被一并打进安装包（CI 干净检出不会有，本地会）。
 */
export function stageAgentBundle({ repoRoot, platformKey, log = console.log }) {
  const { pluginHostBundlePath, glmDir, stagedPluginHostPath } = resolveAgentBundlePaths({
    repoRoot,
    platformKey,
  });
  if (!existsSync(pluginHostBundlePath)) {
    throw new Error(
      `[stage:agent-bundle] plugin host bundle 源产物不存在：${pluginHostBundlePath}`,
    );
  }
  rmSync(glmDir, { recursive: true, force: true });
  mkdirSync(glmDir, { recursive: true });
  copyFileSync(pluginHostBundlePath, stagedPluginHostPath);
  log(`[stage:agent-bundle] staged plugin host only (no Node agent bundle)`);
  return { stagedPluginHostPath };
}
