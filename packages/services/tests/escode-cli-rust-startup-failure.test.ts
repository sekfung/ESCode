import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  ESCodeAgentProcessManager,
  type ESCodeAgentCommand,
} from "../src/escode-agent/escodeAgentProcessManager.js";

// docs/specs/rust-packaging.md「不回退 Node」与「换代等待旧 owner」：
// - Rust 就绪前失败只记录诊断，后续启动仍按选择用 Rust，不改用 Node；
// - 同一 workspace 的新一代 runtime 必须等旧一代退出（释放 owner 锁）后才启动。
const workspace = { workspacePath: tmpdir() };
const crashingRust: ESCodeAgentCommand = {
  runtime: "escode-cli-rust",
  command: process.execPath,
  args: ["-e", "process.exit(3)"],
};

async function waitUntil(predicate: () => boolean, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("condition not reached");
    await new Promise((r) => setTimeout(r, 25));
  }
}

test("a Rust runtime that exits before ready does not switch later starts to Node", async () => {
  const commands: ESCodeAgentCommand[] = [];
  const manager = new ESCodeAgentProcessManager({
    commandResolver: (context) => {
      assert.equal(
        (context as Record<string, unknown>).rustRuntimeFailed,
        undefined,
        "the resolver receives no fallback state",
      );
      commands.push(crashingRust);
      return crashingRust;
    },
  });
  try {
    await manager.getClient(workspace);
    await waitUntil(() => manager.getExistingClient(workspace) === undefined);
    await manager.getClient(workspace);
    assert.equal(commands.length, 2);
    assert.ok(commands.every((command) => command.runtime === "escode-cli-rust"));
  } finally {
    await manager.disposeAllAndWait();
  }
});

test("a replacement runtime starts only after the retiring runtime released the workspace", async () => {
  const dir = await mkdtemp(join(tmpdir(), "escode-runtime-owner-"));
  const owner = join(dir, "owner");
  // 第一代：拿到 owner 标记后关闭 stdout（Host 视为协议关闭并开始回收），但进程仍存活 600ms 才释放标记退出，
  // 模拟 Windows 上回收进程树期间旧 runtime 仍持有 workspace 锁。
  const retiring: ESCodeAgentCommand = {
    runtime: "escode-cli-rust",
    command: process.execPath,
    args: [
      "-e",
      `const fs=require("fs");const m=${JSON.stringify(owner)};fs.writeFileSync(m,"1");` +
        `const release=()=>{try{fs.unlinkSync(m)}catch{}process.exit(0)};process.on("SIGTERM",release);` +
        `process.stdin.resume();setTimeout(()=>{process.stdout.end();setTimeout(release,600)},200);`,
    ],
  };
  // 第二代：与真实 Rust 相同，owner 仍被占用时以 75 退出。
  const replacement: ESCodeAgentCommand = {
    runtime: "escode-cli-rust",
    command: process.execPath,
    args: [
      "-e",
      `const fs=require("fs");if(fs.existsSync(${JSON.stringify(owner)}))process.exit(75);` +
        `process.stdin.resume();process.stdin.on("end",()=>process.exit(0));`,
    ],
  };
  let starts = 0;
  const manager = new ESCodeAgentProcessManager({
    commandResolver: () => (starts++ === 0 ? retiring : replacement),
  });
  try {
    await manager.getClient(workspace);
    await waitUntil(() => manager.getExistingClient(workspace) === undefined);
    const client = await manager.getClient(workspace);
    await new Promise((r) => setTimeout(r, 400));
    assert.equal(starts, 2);
    assert.equal(
      manager.getExistingClient(workspace),
      client,
      "the replacement keeps running instead of exiting on an owned workspace",
    );
  } finally {
    await manager.disposeAllAndWait();
    await rm(dir, { recursive: true, force: true });
  }
});
