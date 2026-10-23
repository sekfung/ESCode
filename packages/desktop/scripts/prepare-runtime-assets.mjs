#!/usr/bin/env node

import { rm } from "node:fs/promises";
import process from "node:process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveNativeSearchReleasePlan } from "../../../scripts/native-search-tools-config.mjs";
import { runCommand } from "../../../scripts/spawn-command.mjs";
import { getTargetPlatform } from "./target-platform.mjs";
import { isRealComputerUseProducerInstalled } from "./computer-use-producer.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopRoot = resolve(scriptDir, "..");
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const target = getTargetPlatform();
const nativeSearchReleasePlan = resolveNativeSearchReleasePlan({
  platform: target.os,
  arch: target.arch,
});
// Windows Chrome 导入入口未启用，默认构建继续编译 helper 会增加 CI 时间和发布签名面。
// 保留显式开关，后续恢复入口时仍可复用既有原生实现和供应链校验。
const shouldPrepareWindowsBrowserImportHelper =
  target.os === "win32" && process.env.ESCODE_ENABLE_WINDOWS_BROWSER_IMPORT === "1";
// Windows Computer Use Helper 由真实 producer 提供；开源占位包没有 Helper runtime，跳过准备。
const shouldPrepareWindowsCuaHelper =
  target.os === "win32" && isRealComputerUseProducerInstalled({ desktopPackageRoot: desktopRoot });
// CUA 权限浮窗的吸附数据源。仅 macOS；缺 swiftc 时脚本内部自行降级为跳过（浮窗 fail-open
// 到屏幕底部，仍可用），所以无条件挂在 darwin 上不会让构建变脆。
const shouldPrepareMacosWindowBounds = target.os === "darwin";

// 本机桌面包内置 agent 的 JS bundle（prepare:agent-bundle），运行时由 app 的 Electron Node runtime 执行。
// 远端跨平台原生二进制仍由上面的 prepare:remote-assets 提供。
// native-search 归档随仓库分发，准备步骤只做本地解包校验，不需要任何下载源配置。
// Rust runtime 默认随包（默认 runtime 已切到 Rust，见 docs/specs/rust-packaging.md）；ESCODE_BUNDLE_RUST_AGENT=0 可关闭，
// 此时安装包只含 Node runtime，桌面端自动用 Node。
const shouldPrepareRustAgent = process.env.ESCODE_BUNDLE_RUST_AGENT !== "0";
const shouldPrepareRustHost = process.env.ESCODE_BUNDLE_RUST_HOST !== "0";
const localRuntimeScripts = [
  "prepare:agent-bundle",
  ...(shouldPrepareRustAgent ? ["prepare:rust-agent"] : []),
  ...(shouldPrepareRustHost ? ["prepare:rust-host"] : []),
  ...(shouldPrepareWindowsCuaHelper ? ["prepare:windows-cua-helper"] : []),
  ...(nativeSearchReleasePlan.enabled ? ["prepare:native-search"] : []),
  ...(shouldPrepareWindowsBrowserImportHelper ? ["prepare:browser-import-helper"] : []),
  ...(shouldPrepareMacosWindowBounds ? ["prepare:macos-window-bounds"] : []),
];

function runTimedPnpmScript(scriptName) {
  const startMs = Date.now();
  console.log(`[ci][timer] prepare-runtime-assets:${scriptName} start`);
  try {
    runCommand(pnpmCommand, [scriptName], {
      cwd: desktopRoot,
      env: process.env,
    });
  } finally {
    console.log(
      `[ci][timer] prepare-runtime-assets:${scriptName} end duration_ms=${Date.now() - startMs}`,
    );
  }
}

// mock-cdn 是从本仓库源码构建的远程 release：开发/e2e 直接把它当本地资源源，
// 安装包内置的离线资源也从这里取（docs/specs/offline-remote-assets.md §7「制品来源」），
// 因此跳过它即同时跳过内置资源（本地 bootstrap 与 --dir 验收沿用 ESCODE_SKIP_REMOTE_ASSETS=1）。
const shouldSkipRemoteAssets = process.env.ESCODE_SKIP_REMOTE_ASSETS === "1";
// 不内置远程资源时，安装包的远程连接回到 CDN 下载语义。
const shouldSkipBundledRemoteAssets =
  shouldSkipRemoteAssets || process.env.ESCODE_SKIP_BUNDLED_REMOTE_ASSETS === "1";

if (!shouldSkipRemoteAssets) {
  // 安装包只内置目标架构的 linux release；远程 glm 组件现含 musl Rust runtime，构建全部 4 个远程平台需要本机
  // 能交叉编译 musl 与 darwin 目标。打包流程未显式指定时只构建会进包的平台（与 release job 同口径）。
  process.env.ESCODE_REMOTE_ASSET_PLATFORMS ||= `linux-${target.arch}`;
  runTimedPnpmScript("prepare:remote-assets");
} else {
  console.log("[prepare-runtime-assets] skip prepare:remote-assets (ESCODE_SKIP_REMOTE_ASSETS=1)");
}

if (!shouldSkipBundledRemoteAssets) {
  // 安装包内置的离线远程资源：取上面 prepare:remote-assets 产出的目标架构 linux release。
  // 本地源缺失或不完整即构建失败，避免产出缺离线能力或与本地契约不一致的包。
  runTimedPnpmScript("prepare:bundled-remote-assets");
} else {
  // electron-builder 只要目录存在就会打包内置资源；残留的上次产物（如曾下载的上游制品）
  // 会让「跳过」名不副实，因此跳过时清掉构建工作区里的旧目录。
  await rm(resolve(desktopRoot, "bundled-remote-assets"), { recursive: true, force: true });
  console.log(
    "[prepare-runtime-assets] skip prepare:bundled-remote-assets " +
      "(ESCODE_SKIP_REMOTE_ASSETS=1 or ESCODE_SKIP_BUNDLED_REMOTE_ASSETS=1); cleared stale bundled-remote-assets",
  );
}

for (const scriptName of localRuntimeScripts) {
  runTimedPnpmScript(scriptName);
}
