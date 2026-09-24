#!/usr/bin/env node

// 可选：把 Rust runtime（apps/zcode-cli-rust）构建并放进 bundled-agents/<platform>/glm，
// 随 resources/glm 一起打包。默认 runtime 仍是 Node；只有 ZCODE_BUNDLE_RUST_AGENT=1 时
// prepare-runtime-assets 才调用本脚本。规则见 docs/specs/rust-packaging.md。

import { chmod, copyFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runCommand } from "../../../scripts/spawn-command.mjs";
import { getTargetPlatform } from "./target-platform.mjs";

const TRIPLES = {
  "win32-x64": "x86_64-pc-windows-msvc",
  "win32-arm64": "aarch64-pc-windows-msvc",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
};

/** 目标平台 → Rust target triple；`ZCODE_RUST_TARGET` 可覆盖（例如本机只有 GNU 工具链）。 */
export function resolveRustTarget(platformKey, override) {
  const triple = override?.trim() || TRIPLES[platformKey];
  if (!triple) throw new Error(`No Rust target for desktop platform ${platformKey}`);
  return triple;
}

export function rustBinaryFileName(os) {
  return os === "win32" ? "zcode-cli-rust.exe" : "zcode-cli-rust";
}

async function main() {
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const desktopRoot = resolve(scriptDir, "..");
  const workspace = resolve(desktopRoot, "..", "..", "apps", "zcode-cli-rust");
  const target = getTargetPlatform();
  const triple = resolveRustTarget(target.key, process.env.ZCODE_RUST_TARGET);
  const identity = process.env.ZCODE_RUST_CODESIGN_IDENTITY?.trim();
  if (target.os === "darwin" && !identity) {
    // glm 目录被 electron-builder signIgnore；未签名的 Mach-O 无法通过公证，不能产出这样的包。
    throw new Error("Bundling the Rust agent for macOS requires ZCODE_RUST_CODESIGN_IDENTITY");
  }
  const cargoArgs = ["build", "--release", "--locked", "--target", triple, "-p", "zcode-cli-rust"];
  if (process.env.ZCODE_RUST_OFFLINE === "1") cargoArgs.push("--offline");
  runCommand("cargo", cargoArgs, { cwd: workspace, env: process.env });

  const file = rustBinaryFileName(target.os);
  const source = resolve(workspace, "target", triple, "release", file);
  const destinationDir = resolve(desktopRoot, "bundled-agents", target.key, "glm");
  await mkdir(destinationDir, { recursive: true });
  const destination = resolve(destinationDir, file);
  await copyFile(source, destination);
  if (target.os !== "win32") await chmod(destination, 0o755);
  if (target.os === "darwin") {
    runCommand(
      "codesign",
      ["--force", "--options", "runtime", "--timestamp", "--sign", identity, destination],
      { cwd: desktopRoot, env: process.env },
    );
  }
  console.log(`[prepare:rust-agent] ${triple} -> ${destination}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
