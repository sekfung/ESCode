#!/usr/bin/env node

// 把 App 级 Rust Host（apps/escode-host）构建并放入 bundled-tools/<platform>/escode-host，
// 随 electron-builder extraResources 复制到 resources/escode-host。
// 默认随包（ESCODE_BUNDLE_RUST_HOST=0 关闭）。规则见 docs/specs/rust-u2-window-host.md §5-10。

import { chmod, copyFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runCommand } from "../../../scripts/spawn-command.mjs";
import { getTargetPlatform } from "./target-platform.mjs";
import { resolveRustTarget } from "./prepare-rust-agent.mjs";

function hostBinaryFileName(os) {
  return os === "win32" ? "escode-host.exe" : "escode-host";
}

async function main() {
  if (process.env.ESCODE_BUNDLE_RUST_HOST === "0") {
    console.log("[prepare:rust-host] skipped (ESCODE_BUNDLE_RUST_HOST=0)");
    return;
  }
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const desktopRoot = resolve(scriptDir, "..");
  const workspace = resolve(desktopRoot, "..", "..", "apps", "escode-host");
  const target = getTargetPlatform();
  const triple = resolveRustTarget(target.key, process.env.ESCODE_RUST_TARGET);
  const signMac = target.os === "darwin" && process.env.ESCODE_ENABLE_MAC_SIGN === "1";
  const identity = (
    process.env.ESCODE_RUST_CODESIGN_IDENTITY ||
    process.env.APPLE_SIGNING_IDENTITY ||
    process.env.CSC_NAME ||
    ""
  ).trim();
  if (signMac && !identity) {
    throw new Error("Signed macOS builds need APPLE_SIGNING_IDENTITY/CSC_NAME for the Rust Host");
  }
  const cargoArgs = ["build", "--release", "--locked", "--target", triple, "-p", "escode-host"];
  if (process.env.ESCODE_RUST_OFFLINE === "1") cargoArgs.push("--offline");
  runCommand("cargo", cargoArgs, { cwd: workspace, env: process.env });

  const file = hostBinaryFileName(target.os);
  const source = resolve(workspace, "target", triple, "release", file);
  const destinationDir = resolve(desktopRoot, "bundled-tools", target.key, "escode-host");
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
  console.log(`[prepare:rust-host] ${triple} -> ${destination}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
