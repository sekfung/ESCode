import assert from "node:assert/strict";
import test from "node:test";
import { ESCODE_AGENT_RUNTIME } from "@escode/shared";
import { resolveDefaultESCodeAgentCommand } from "../src/escode-agent/escodeAgentProcessManager.js";

// docs/specs/rust-packaging.md：Node runtime 退场后 Rust 是唯一 agent runtime。
// 选择只在 resolveDefaultESCodeAgentCommand 一处决定，不再读取 ESCODE_AGENT_SERVER_RUNTIME。
const KEYS = [
  "ESCODE_AGENT_SERVER_RUNTIME",
  "ESCODE_AGENT_SERVER_COMMAND",
  "ESCODE_AGENT_SERVER_ARGS_JSON",
  "ESCODE_AGENT_SERVER_CWD",
  "ESCODE_DESKTOP_PACKAGED",
  "ESCODE_REMOTE_PACKAGED_RUNTIME",
] as const;

function withEnv<T>(env: Partial<Record<(typeof KEYS)[number], string>>, run: () => T): T {
  const saved = KEYS.map((k) => [k, process.env[k]] as const);
  for (const k of KEYS) delete process.env[k];
  Object.assign(process.env, env);
  try {
    return run();
  } finally {
    for (const [k, v] of saved) {
      if (v === undefined) delete process.env[k];
      else process.env[k] = v;
    }
  }
}

const context = { workspacePath: "/work/space", workspaceKey: "/work/space" };
const noDev = { findDevRustBinary: () => null };
const bundled = { ...noDev, findRustBinary: () => "/res/glm/escode-cli-rust" };
const missing = { ...noDev, findRustBinary: () => null };

test("development resolves the local cargo-built Rust binary with Host-supplied cwd", () => {
  const resolved = withEnv({}, () =>
    resolveDefaultESCodeAgentCommand(context, {
      findDevRustBinary: () => "/repo/apps/escode-cli-rust/target/release/escode-cli-rust",
      findRustBinary: () => null,
    }),
  );
  // 插件宿主 env 另有用例覆盖（取决于本机是否有 Electron / 插件宿主产物）。
  const { env: _pluginHost, ...command } = resolved ?? ({} as any);
  assert.deepEqual(command, {
    runtime: "escode-cli-rust",
    command: "/repo/apps/escode-cli-rust/target/release/escode-cli-rust",
    storagePreparationMode: "process",
    supportsStorageStartup: true,
    args: ["app-server", "--stdio", "--cwd", "/work/space"],
    cwd: "/work/space",
  });
});

// 桌面安装包：Main 注入 ESCODE_DESKTOP_PACKAGED=1，不探测仓库内 target 产物。
const PACKAGED = { ESCODE_DESKTOP_PACKAGED: "1" } as const;

test("the default runtime is the bundled Rust binary in an installed app", () => {
  const command = withEnv(PACKAGED, () =>
    resolveDefaultESCodeAgentCommand(context, {
      ...bundled,
      // 安装包不能因为从仓库目录启动就误用开发产物。
      findDevRustBinary: () => "/repo/apps/escode-cli-rust/target/release/escode-cli-rust",
    }),
  );
  assert.equal(command?.command, "/res/glm/escode-cli-rust");
  assert.equal(command?.runtime, "escode-cli-rust");
  assert.deepEqual(command?.args, ["app-server", "--stdio", "--cwd", "/work/space"]);
});

test("an installed app without the Rust binary reports an error", () => {
  assert.throws(
    () => withEnv(PACKAGED, () => resolveDefaultESCodeAgentCommand(context, missing)),
    /Rust runtime \(escode-cli-rust\) is not installed/,
  );
});

// docs/specs/rust-packaging.md「远程环境使用 Rust」：桌面安装包部署的远端 server 同样默认 Rust、缺二进制报错。
const REMOTE_PACKAGED = { ESCODE_REMOTE_PACKAGED_RUNTIME: "1" } as const;

test("a remote server deployed by an installed app defaults to the Rust runtime", () => {
  const command = withEnv(REMOTE_PACKAGED, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.equal(command?.command, "/res/glm/escode-cli-rust");
  assert.equal(command?.runtime, "escode-cli-rust");
});

test("a remote server deployed by an installed app reports a missing Rust binary", () => {
  assert.throws(
    () => withEnv(REMOTE_PACKAGED, () => resolveDefaultESCodeAgentCommand(context, missing)),
    /Rust runtime \(escode-cli-rust\) is not installed/,
  );
});

test("development without a cargo build or bundled binary fails instead of switching runtime", () => {
  assert.throws(
    () => withEnv({}, () => resolveDefaultESCodeAgentCommand(context, missing)),
    /Run pnpm build:escode-cli-rust in development/,
  );
});

test("an explicit command wins and keeps the Rust contract", () => {
  const command = withEnv({ ESCODE_AGENT_SERVER_COMMAND: "/custom/rust" }, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.equal(command?.command, "/custom/rust");
  assert.deepEqual(command?.args, ["app-server", "--stdio", "--cwd", "/work/space"]);

  const withCwd = withEnv(
    {
      ESCODE_AGENT_SERVER_COMMAND: "/custom/rust",
      ESCODE_AGENT_SERVER_CWD: "/custom/cwd",
    },
    () => resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.equal(withCwd?.cwd, "/custom/cwd");
});

test("a stale ESCODE_AGENT_SERVER_RUNTIME=node no longer selects or allows a Node runtime", () => {
  // 环境变量已删除：残留值不应改变选择，也不应再报 Unsupported。
  const command = withEnv(
    { ESCODE_AGENT_SERVER_RUNTIME: "node", ESCODE_AGENT_SERVER_COMMAND: "/custom/agent" },
    () => resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.equal(command?.command, "/custom/agent");
  assert.deepEqual(command?.args, ["app-server", "--stdio", "--cwd", "/work/space"]);
});

test("the Host still owns --cwd for Rust", () => {
  assert.throws(
    () =>
      withEnv(
        { ESCODE_AGENT_SERVER_ARGS_JSON: JSON.stringify(["app-server", "--cwd", "/x"]) },
        () => resolveDefaultESCodeAgentCommand(context, bundled),
      ),
    /--cwd is supplied by the Host/,
  );
});

test("the descriptor names the Rust binary per platform", () => {
  assert.deepEqual(ESCODE_AGENT_RUNTIME.resolveRustBinarySegments("win32"), [
    "escode-cli-rust.exe",
  ]);
  assert.deepEqual(ESCODE_AGENT_RUNTIME.resolveRustBinarySegments("darwin"), ["escode-cli-rust"]);
});

test("desktop packaging maps each platform to a Rust target and binary name", async () => {
  const { resolveRustTarget, rustBinaryFileName } =
    await import("../../desktop/scripts/prepare-rust-agent.mjs");
  assert.equal(resolveRustTarget("win32-x64"), "x86_64-pc-windows-msvc");
  assert.equal(resolveRustTarget("darwin-arm64"), "aarch64-apple-darwin");
  assert.equal(resolveRustTarget("linux-x64"), "x86_64-unknown-linux-gnu");
  assert.equal(resolveRustTarget("win32-x64", "x86_64-pc-windows-gnu"), "x86_64-pc-windows-gnu");
  assert.throws(() => resolveRustTarget("freebsd-x64"), /No Rust target/);
  assert.equal(rustBinaryFileName("win32"), "escode-cli-rust.exe");
  assert.equal(rustBinaryFileName("linux"), "escode-cli-rust");
});

// 端到端：不注入 deps，走真实解析链，确认开发态能发现本地 cargo 产物并完成一轮。
// 未构建时跳过（如实标注为未覆盖，而不是伪造通过）。
test("the Host resolves the local cargo-built Rust binary and it completes a turn", async (t) => {
  const { findESCodeAgentDevRustBinary } =
    await import("../src/runtime-tools/providerRuntimeResolver.js");
  const devBinary = findESCodeAgentDevRustBinary();
  if (!devBinary) {
    t.skip("no local Rust binary (run pnpm build:escode-cli-rust)");
    return;
  }
  const resolved = withEnv({}, () =>
    resolveDefaultESCodeAgentCommand({ workspacePath: process.cwd(), workspaceKey: process.cwd() }),
  );
  assert.equal(resolved?.command, devBinary);
  const { fixture } = await import("./escode-cli-rust-fixture.js");
  const f = await fixture({ binary: resolved!.command, mode: "yolo" });
  try {
    const h = f.start();
    const id = await h.create("hello from the local rust runtime");
    await h.subscribe(`conversation/${id}`);
    await h.completed(id);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});

test("Rust commands are marked so the manager can attribute startup failures", () => {
  const rust = withEnv({}, () => resolveDefaultESCodeAgentCommand(context, bundled));
  assert.equal(rust?.runtime, "escode-cli-rust");
  const explicitRust = withEnv({ ESCODE_AGENT_SERVER_COMMAND: "/custom/rust" }, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.equal(explicitRust?.runtime, "escode-cli-rust");
});
