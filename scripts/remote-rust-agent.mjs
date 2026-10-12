// 远程 release 的 Rust runtime（docs/specs/rust-packaging.md「远程环境使用 Rust」）：
// 把各远程平台的 escode-cli-rust 放进 glm/<platform>/，与 zcode.cjs 同一组件。
// Linux 远端用 musl 静态链接，不依赖远端 glibc 版本。

import { chmodSync, copyFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { runCommand } from "./spawn-command.mjs";

export const REMOTE_RUST_BINARY_NAME = "escode-cli-rust";

const REMOTE_RUST_TARGETS = {
  "linux-x64": "x86_64-unknown-linux-musl",
  "linux-arm64": "aarch64-unknown-linux-musl",
  "darwin-x64": "x86_64-apple-darwin",
  "darwin-arm64": "aarch64-apple-darwin",
};

/** 远程平台 → Rust target triple。 */
export function resolveRemoteRustTarget(platformKey) {
  const target = REMOTE_RUST_TARGETS[platformKey];
  if (!target) throw new Error(`No remote Rust target for platform ${platformKey}`);
  return target;
}

/**
 * 解析某个远程平台的 Rust 二进制来源：`ESCODE_REMOTE_RUST_BINARIES_DIR/<platform>/escode-cli-rust` 优先
 * （CI 由 rust-runtime job 产物提供）；未设置时返回 cargo 构建计划。设置了目录却缺文件属于配置错误，直接失败。
 */
export function resolveRemoteRustBinarySource(platformKey, { env = process.env, rootDir }) {
  const dir = env.ESCODE_REMOTE_RUST_BINARIES_DIR?.trim();
  if (dir) {
    const path = join(dir, platformKey, REMOTE_RUST_BINARY_NAME);
    if (!existsSync(path)) {
      throw new Error(
        `[remote-rust-agent] ESCODE_REMOTE_RUST_BINARIES_DIR is set but ${path} is missing`,
      );
    }
    return { kind: "prebuilt", path };
  }
  const target = resolveRemoteRustTarget(platformKey);
  const workspace = join(rootDir, "apps", "escode-cli-rust");
  return {
    kind: "cargo",
    target,
    workspace,
    path: join(workspace, "target", target, "release", REMOTE_RUST_BINARY_NAME),
  };
}

/** 准备并复制到 glm 组件目录；拿不到二进制即抛错（远程默认 runtime 是 Rust，不产出缺二进制的远程包）。 */
export function stageRemoteRustAgent({ platformKey, glmDir, rootDir, env = process.env }) {
  const source = resolveRemoteRustBinarySource(platformKey, { env, rootDir });
  if (source.kind === "cargo") {
    const args = ["build", "--release", "--locked", "--target", source.target, "-p", "escode-cli-rust"];
    if (env.ESCODE_RUST_OFFLINE === "1") args.push("--offline");
    try {
      runCommand("cargo", args, { cwd: source.workspace, env });
    } catch (error) {
      throw new Error(
        `[remote-rust-agent] cannot build ${source.target} for ${platformKey}. ` +
          "Install the target (and musl-tools for Linux), or provide prebuilt binaries via " +
          `ESCODE_REMOTE_RUST_BINARIES_DIR=<dir>/<platform>/${REMOTE_RUST_BINARY_NAME}. ` +
          `Cause: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    if (!existsSync(source.path)) {
      throw new Error(`[remote-rust-agent] cargo finished but ${source.path} is missing`);
    }
  }
  const destination = join(glmDir, REMOTE_RUST_BINARY_NAME);
  copyFileSync(source.path, destination);
  chmodSync(destination, 0o755);
  return destination;
}
