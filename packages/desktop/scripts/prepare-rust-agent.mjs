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
  // 与应用本身的签名开关一致：ZCODE_ENABLE_MAC_SIGN=1 时应用会签名并公证，glm 目录却被
  // electron-builder signIgnore，Rust 二进制必须用同一身份自行签名，否则公证失败；
  // 未开启时整个应用都不签名（CI 未签名包），二进制保持未签名。
  const signMac = target.os === "darwin" && process.env.ZCODE_ENABLE_MAC_SIGN === "1";
  const identity = (
    process.env.ZCODE_RUST_CODESIGN_IDENTITY ||
    process.env.APPLE_SIGNING_IDENTITY ||
    process.env.CSC_NAME ||
    ""
  ).trim();
  if (signMac && !identity) {
    throw new Error("Signed macOS builds need APPLE_SIGNING_IDENTITY/CSC_NAME for the Rust agent");
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
  if (signMac) {
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
