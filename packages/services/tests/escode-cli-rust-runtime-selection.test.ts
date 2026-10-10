import assert from "node:assert/strict";
import test from "node:test";
import { ESCODE_AGENT_RUNTIME } from "@escode/shared";
import { resolveDefaultESCodeAgentCommand } from "../src/escode-agent/escodeAgentProcessManager.js";

// docs/specs/rust-packaging.md：runtime 选择只在 resolveDefaultESCodeAgentCommand 一处决定。
const KEYS = [
  "ESCODE_AGENT_SERVER_RUNTIME",
  "ESCODE_AGENT_SERVER_COMMAND",
  "ESCODE_AGENT_SERVER_ARGS_JSON",
  "ESCODE_AGENT_SERVER_CWD",
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
const bundled = { findRustBinary: () => "/res/glm/escode-cli-rust" };
const missing = { findRustBinary: () => null };

test("selecting escode-cli-rust uses the bundled binary with Host-supplied cwd and storage startup", () => {
  const resolved = withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust" }, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  // 插件宿主 env 另有用例覆盖（取决于本机是否有 Node 入口产物）。
  const { env: _pluginHost, ...command } = resolved ?? ({} as any);
  assert.deepEqual(command, {
    runtime: "escode-cli-rust",
    command: "/res/glm/escode-cli-rust",
    storagePreparationMode: "process",
    supportsStorageStartup: true,
    args: ["app-server", "--stdio", "--cwd", "/work/space"],
    cwd: "/work/space",
  });
});

test("the Rust command carries the Node plugin host used by official plugin seeding", () => {
  // docs/specs/rust-official-plugin-seed.md：与 Node runtime 相同的 Electron-as-Node 与 escode.cjs 入口。
  const node = withEnv({}, () => resolveDefaultESCodeAgentCommand(context, bundled));
  const rust = withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust" }, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  const entrypoint = node?.args?.[0];
  if (!entrypoint?.endsWith(".cjs")) {
    assert.equal(rust?.env, undefined);
    return;
  }
  assert.deepEqual(rust?.env, {
    ESCODE_PLUGIN_HOST_EXEC_PATH: node!.command,
    ESCODE_PLUGIN_HOST_ENTRYPOINT: entrypoint,
  });
});

// 默认 runtime 为 Rust（安装态，没有 monorepo 开发入口）：不设变量即用随包二进制；开发态与显式 node 保持 Node。
const installed = { ...bundled, findDevCommand: () => null };
const installedMissing = { ...missing, findDevCommand: () => null };
const dev = {
  ...bundled,
  findDevCommand: () => ({ command: "/dev/node", args: ["/dev/escode.cjs", "app-server", "--stdio"] }),
};

test("the default runtime is the bundled Rust binary in an installed app", () => {
  const command = withEnv({}, () => resolveDefaultESCodeAgentCommand(context, installed));
  assert.equal(command?.command, "/res/glm/escode-cli-rust");
  assert.equal(command?.runtime, "escode-cli-rust");
  assert.deepEqual(command?.args, ["app-server", "--stdio", "--cwd", "/work/space"]);
});

test("the default falls back to Node when the binary is missing, in development, after a Rust failure, or when node is chosen", () => {
  const notRust = (command: ReturnType<typeof resolveDefaultESCodeAgentCommand>) =>
    assert.notEqual(command?.command, "/res/glm/escode-cli-rust");
  notRust(withEnv({}, () => resolveDefaultESCodeAgentCommand(context, installedMissing)));
  notRust(withEnv({}, () => resolveDefaultESCodeAgentCommand(context, dev)));
  notRust(
    withEnv({}, () => resolveDefaultESCodeAgentCommand({ ...context, rustRuntimeFailed: true }, installed)),
  );
  notRust(withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "node" }, () => resolveDefaultESCodeAgentCommand(context, installed)));
});

test("a missing bundled Rust binary falls back to the Node runtime instead of failing", () => {
  const command = withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust" }, () =>
    resolveDefaultESCodeAgentCommand(context, missing),
  );
  assert.notEqual(command?.command, "/res/glm/escode-cli-rust");
  assert(!command?.args?.includes("--cwd"), "Node fallback must not receive Rust-only --cwd");
});

test("an explicit command still wins and keeps the Rust contract when the runtime is Rust", () => {
  const command = withEnv(
    { ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust", ESCODE_AGENT_SERVER_COMMAND: "/custom/rust" },
    () => resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.equal(command?.command, "/custom/rust");
  assert.deepEqual(command?.args, ["app-server", "--stdio", "--cwd", "/work/space"]);
  const node = withEnv({ ESCODE_AGENT_SERVER_COMMAND: "/custom/node" }, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.deepEqual(node, {
    command: "/custom/node",
    args: ["app-server", "--stdio"],
    cwd: "/work/space",
  });
});

test("node is an accepted explicit runtime and never picks the Rust binary; unknown values throw", () => {
  const command = withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "node" }, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.notEqual(command?.command, "/res/glm/escode-cli-rust");
  assert.throws(
    () =>
      withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "deno" }, () =>
        resolveDefaultESCodeAgentCommand(context, bundled),
      ),
    /Unsupported ESCODE_AGENT_SERVER_RUNTIME/,
  );
});

test("the Host still owns --cwd for Rust", () => {
  assert.throws(
    () =>
      withEnv(
        {
          ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust",
          ESCODE_AGENT_SERVER_ARGS_JSON: JSON.stringify(["app-server", "--cwd", "/x"]),
        },
        () => resolveDefaultESCodeAgentCommand(context, bundled),
      ),
    /--cwd is supplied by the Host/,
  );
});

test("the descriptor names the Rust binary per platform", () => {
  assert.deepEqual(ESCODE_AGENT_RUNTIME.resolveRustBinarySegments("win32"), ["escode-cli-rust.exe"]);
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

// 端到端：Host 真实解析链（不注入）找到 prepare:rust-agent 放进 bundled-agents 的二进制，且它能完成一轮。
// 默认构建不随包 Rust，此时跳过（如实标注为未覆盖，而不是伪造通过）。
test("the Host resolves a staged bundled Rust binary and it completes a turn", async (t) => {
  const { findESCodeAgentRustBinary } =
    await import("../src/runtime-tools/providerRuntimeResolver.js");
  if (!findESCodeAgentRustBinary()) {
    t.skip("no staged Rust binary (run prepare:rust-agent with ESCODE_BUNDLE_RUST_AGENT=1)");
    return;
  }
  const resolved = withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust" }, () =>
    resolveDefaultESCodeAgentCommand({ workspacePath: process.cwd(), workspaceKey: process.cwd() }),
  );
  assert.equal(resolved?.command, findESCodeAgentRustBinary());
  const { fixture } = await import("./escode-cli-rust-fixture.js");
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
  const bundledRust = withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust" }, () =>
    resolveDefaultESCodeAgentCommand(failed, bundled),
  );
  assert.notEqual(bundledRust?.command, "/res/glm/escode-cli-rust");
  assert.equal(bundledRust?.runtime, undefined);
  const explicitRust = withEnv(
    { ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust", ESCODE_AGENT_SERVER_COMMAND: "/custom/rust" },
    () => resolveDefaultESCodeAgentCommand(failed, bundled),
  );
  assert.notEqual(explicitRust?.command, "/custom/rust");
  // Rust 命令都带标记，manager 据此判断失败的是 Rust。
  const rust = withEnv({ ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust" }, () =>
    resolveDefaultESCodeAgentCommand(context, bundled),
  );
  assert.equal(rust?.runtime, "escode-cli-rust");
});
