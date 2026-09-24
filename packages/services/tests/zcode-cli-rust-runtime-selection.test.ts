import assert from "node:assert/strict";
import test from "node:test";
import { ZCODE_AGENT_RUNTIME } from "@zcode/shared";
import { resolveDefaultZCodeAgentCommand } from "../src/zcode-agent/zcodeAgentProcessManager.js";

// docs/specs/rust-packaging.md：runtime 选择只在 resolveDefaultZCodeAgentCommand 一处决定。
const KEYS = [
  "ZCODE_AGENT_SERVER_RUNTIME",
  "ZCODE_AGENT_SERVER_COMMAND",
  "ZCODE_AGENT_SERVER_ARGS_JSON",
  "ZCODE_AGENT_SERVER_CWD",
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

const context = { workspacePath: "/work/space" };
const bundled = { findRustBinary: () => "/res/glm/zcode-cli-rust" };
const missing = { findRustBinary: () => null };

test("selecting zcode-cli-rust uses the bundled binary with Host-supplied cwd and storage startup", () => {
  const command = withEnv({ ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust" }, () =>
    resolveDefaultZCodeAgentCommand(context, bundled),
  );
  assert.deepEqual(command, {
    runtime: "zcode-cli-rust",
    command: "/res/glm/zcode-cli-rust",
    storagePreparationMode: "process",
    supportsStorageStartup: true,
    args: ["app-server", "--stdio", "--cwd", "/work/space"],
    cwd: "/work/space",
  });
});

test("a missing bundled Rust binary falls back to the Node runtime instead of failing", () => {
  const command = withEnv({ ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust" }, () =>
    resolveDefaultZCodeAgentCommand(context, missing),
  );
  assert.notEqual(command?.command, "/res/glm/zcode-cli-rust");
  assert(!command?.args?.includes("--cwd"), "Node fallback must not receive Rust-only --cwd");
});

test("an explicit command still wins and keeps the Rust contract when the runtime is Rust", () => {
  const command = withEnv(
    { ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust", ZCODE_AGENT_SERVER_COMMAND: "/custom/rust" },
    () => resolveDefaultZCodeAgentCommand(context, bundled),
  );
  assert.equal(command?.command, "/custom/rust");
  assert.deepEqual(command?.args, ["app-server", "--stdio", "--cwd", "/work/space"]);
  const node = withEnv({ ZCODE_AGENT_SERVER_COMMAND: "/custom/node" }, () =>
    resolveDefaultZCodeAgentCommand(context, bundled),
  );
  assert.deepEqual(node, {
    command: "/custom/node",
    args: ["app-server", "--stdio"],
    cwd: "/work/space",
  });
});

test("node is an accepted explicit runtime and never picks the Rust binary; unknown values throw", () => {
  const command = withEnv({ ZCODE_AGENT_SERVER_RUNTIME: "node" }, () =>
    resolveDefaultZCodeAgentCommand(context, bundled),
  );
  assert.notEqual(command?.command, "/res/glm/zcode-cli-rust");
  assert.throws(
    () =>
      withEnv({ ZCODE_AGENT_SERVER_RUNTIME: "deno" }, () =>
        resolveDefaultZCodeAgentCommand(context, bundled),
      ),
    /Unsupported ZCODE_AGENT_SERVER_RUNTIME/,
  );
});

test("the Host still owns --cwd for Rust", () => {
  assert.throws(
    () =>
      withEnv(
        {
          ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust",
          ZCODE_AGENT_SERVER_ARGS_JSON: JSON.stringify(["app-server", "--cwd", "/x"]),
        },
        () => resolveDefaultZCodeAgentCommand(context, bundled),
      ),
    /--cwd is supplied by the Host/,
  );
});

test("the descriptor names the Rust binary per platform", () => {
  assert.deepEqual(ZCODE_AGENT_RUNTIME.resolveRustBinarySegments("win32"), ["zcode-cli-rust.exe"]);
  assert.deepEqual(ZCODE_AGENT_RUNTIME.resolveRustBinarySegments("darwin"), ["zcode-cli-rust"]);
});

test("desktop packaging maps each platform to a Rust target and binary name", async () => {
  const { resolveRustTarget, rustBinaryFileName } =
    await import("../../desktop/scripts/prepare-rust-agent.mjs");
  assert.equal(resolveRustTarget("win32-x64"), "x86_64-pc-windows-msvc");
  assert.equal(resolveRustTarget("darwin-arm64"), "aarch64-apple-darwin");
  assert.equal(resolveRustTarget("linux-x64"), "x86_64-unknown-linux-gnu");
  assert.equal(resolveRustTarget("win32-x64", "x86_64-pc-windows-gnu"), "x86_64-pc-windows-gnu");
  assert.throws(() => resolveRustTarget("freebsd-x64"), /No Rust target/);
  assert.equal(rustBinaryFileName("win32"), "zcode-cli-rust.exe");
  assert.equal(rustBinaryFileName("linux"), "zcode-cli-rust");
});

// 端到端：Host 真实解析链（不注入）找到 prepare:rust-agent 放进 bundled-agents 的二进制，且它能完成一轮。
// 默认构建不随包 Rust，此时跳过（如实标注为未覆盖，而不是伪造通过）。
test("the Host resolves a staged bundled Rust binary and it completes a turn", async (t) => {
  const { findZCodeAgentRustBinary } =
    await import("../src/runtime-tools/providerRuntimeResolver.js");
  if (!findZCodeAgentRustBinary()) {
    t.skip("no staged Rust binary (run prepare:rust-agent with ZCODE_BUNDLE_RUST_AGENT=1)");
    return;
  }
  const resolved = withEnv({ ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust" }, () =>
    resolveDefaultZCodeAgentCommand({ workspacePath: process.cwd() }),
  );
  assert.equal(resolved?.command, findZCodeAgentRustBinary());
  const { fixture } = await import("./zcode-cli-rust-fixture.js");
  const f = await fixture({ binary: resolved!.command, mode: "yolo" });
  try {
    const h = f.start();
    const id = await h.create("hello from the bundled runtime");
    await h.subscribe(`conversation/${id}`);
    await h.completed(id);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});

test("after a Rust startup failure the resolver ignores both the bundled and the explicit Rust command", () => {
  const failed = { ...context, rustRuntimeFailed: true };
  const bundledRust = withEnv({ ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust" }, () =>
    resolveDefaultZCodeAgentCommand(failed, bundled),
  );
  assert.notEqual(bundledRust?.command, "/res/glm/zcode-cli-rust");
  assert.equal(bundledRust?.runtime, undefined);
  const explicitRust = withEnv(
    { ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust", ZCODE_AGENT_SERVER_COMMAND: "/custom/rust" },
    () => resolveDefaultZCodeAgentCommand(failed, bundled),
  );
  assert.notEqual(explicitRust?.command, "/custom/rust");
  // Rust 命令都带标记，manager 据此判断失败的是 Rust。
  const rust = withEnv({ ZCODE_AGENT_SERVER_RUNTIME: "zcode-cli-rust" }, () =>
    resolveDefaultZCodeAgentCommand(context, bundled),
  );
  assert.equal(rust?.runtime, "zcode-cli-rust");
});
