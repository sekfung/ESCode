import { mkdirSync, statSync, writeFileSync, readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  REMOTE_RUST_BINARY_NAME,
  resolveRemoteRustBinarySource,
  resolveRemoteRustTarget,
  stageRemoteRustAgent,
} from "../../../scripts/remote-rust-agent.mjs";

// docs/specs/rust-packaging.md「远程环境使用 Rust」：远程 release 的 glm 组件携带 escode-cli-rust。
const roots: string[] = [];
async function tempRoot() {
  const root = await mkdtemp(join(tmpdir(), "escode-remote-rust-"));
  roots.push(root);
  return root;
}
afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe("remote Rust runtime staging", () => {
  it("maps Linux remotes to static musl targets and macOS remotes to darwin targets", () => {
    expect(resolveRemoteRustTarget("linux-x64")).toBe("x86_64-unknown-linux-musl");
    expect(resolveRemoteRustTarget("linux-arm64")).toBe("aarch64-unknown-linux-musl");
    expect(resolveRemoteRustTarget("darwin-arm64")).toBe("aarch64-apple-darwin");
    expect(resolveRemoteRustTarget("darwin-x64")).toBe("x86_64-apple-darwin");
    expect(() => resolveRemoteRustTarget("win32-x64")).toThrow(/No remote Rust target/);
  });

  it("prefers prebuilt binaries and copies them as executables into the glm component", async () => {
    const root = await tempRoot();
    const prebuilt = join(root, "prebuilt");
    mkdirSync(join(prebuilt, "linux-x64"), { recursive: true });
    writeFileSync(join(prebuilt, "linux-x64", REMOTE_RUST_BINARY_NAME), "rust");
    const glmDir = join(root, "glm");
    mkdirSync(glmDir);
    const env = { ESCODE_REMOTE_RUST_BINARIES_DIR: prebuilt };

    expect(resolveRemoteRustBinarySource("linux-x64", { env, rootDir: root })).toEqual({
      kind: "prebuilt",
      path: join(prebuilt, "linux-x64", REMOTE_RUST_BINARY_NAME),
    });
    const staged = stageRemoteRustAgent({ platformKey: "linux-x64", glmDir, rootDir: root, env });
    expect(staged).toBe(join(glmDir, REMOTE_RUST_BINARY_NAME));
    expect(readFileSync(staged, "utf8")).toBe("rust");
    if (process.platform !== "win32") expect(statSync(staged).mode & 0o777).toBe(0o755);
  });

  it("fails when the prebuilt directory lacks the platform binary instead of shipping without Rust", async () => {
    const root = await tempRoot();
    const env = { ESCODE_REMOTE_RUST_BINARIES_DIR: join(root, "empty") };
    expect(() => resolveRemoteRustBinarySource("linux-arm64", { env, rootDir: root })).toThrow(
      /ESCODE_REMOTE_RUST_BINARIES_DIR is set but/,
    );
  });

  it("plans a cargo build for the remote target when no prebuilt directory is given", () => {
    const source = resolveRemoteRustBinarySource("linux-x64", { env: {}, rootDir: "/repo" });
    expect(source).toEqual({
      kind: "cargo",
      target: "x86_64-unknown-linux-musl",
      workspace: join("/repo", "apps", "escode-cli-rust"),
      path: join(
        "/repo",
        "apps",
        "escode-cli-rust",
        "target",
        "x86_64-unknown-linux-musl",
        "release",
        REMOTE_RUST_BINARY_NAME,
      ),
    });
  });
});
