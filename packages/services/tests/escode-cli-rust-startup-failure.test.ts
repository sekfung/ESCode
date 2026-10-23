import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import {
  ESCodeAgentProcessManager,
  type ESCodeAgentCommand,
} from "../src/escode-agent/escodeAgentProcessManager.js";
import type { RuntimeProcessLifecycleReporter } from "../src/process/runtimeProcessLifecycle.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-packaging.md「不回退 Node」与「换代等待旧 owner」：
// - Rust 就绪前失败只记录诊断，后续启动仍按选择用 Rust，不改用 Node；
// - 同一 workspace 的新一代 runtime 必须等旧一代退出（释放 owner 锁）后才启动。
// Node 在这里只作为「就绪前崩溃 / 占用 owner 锁」的失败进程；正常跑只读冻结的
// 错误与退出形状（`ESCODE_RECORD_CLI_GOLDEN=1` 时才真实启动 Node）。
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

/** 只保留可复现的退出形状：pid、时间戳与进程路径每次不同，不参与比较。 */
function lifecycleRecorder() {
  const events: Record<string, unknown>[] = [];
  const reporter: RuntimeProcessLifecycleReporter = {
    onSpawn(event) {
      events.push({ type: "spawn", runtimeGeneration: event.runtimeGeneration });
    },
    onExit(event) {
      events.push({
        type: "exit",
        runtimeGeneration: event.runtimeGeneration,
        exitCode: event.exitCode,
        signal: event.signal,
        terminationKind: event.terminationKind,
        runtimeReady: event.runtimeReady,
        stderrLineCount: event.stderrLineCount,
      });
    },
  };
  const exits = () => events.filter((event) => event.type === "exit");
  return { events, exits, reporter };
}

async function observeCrashBeforeReady() {
  const commands: ESCodeAgentCommand[] = [];
  const resolverFallbackStates: unknown[] = [];
  const lifecycle = lifecycleRecorder();
  const manager = new ESCodeAgentProcessManager({
    processLifecycleReporter: lifecycle.reporter,
    commandResolver: (context) => {
      resolverFallbackStates.push(
        (context as { rustRuntimeFailed?: unknown }).rustRuntimeFailed ?? null,
      );
      commands.push(crashingRust);
      return crashingRust;
    },
  });
  try {
    await manager.getClient(workspace);
    await waitUntil(() => manager.getExistingClient(workspace) === undefined);
    await waitUntil(() => lifecycle.exits().length === 1);
    const second = await manager.getClient(workspace);
    await waitUntil(() => manager.getExistingClient(workspace) === undefined);
    await waitUntil(() => lifecycle.exits().length === 2);
    return canonical({
      resolverFallbackStates,
      starts: commands.length,
      runtimes: commands.map((command) => command.runtime),
      secondClientReturned: second !== undefined,
      lifecycle: lifecycle.events,
    });
  } finally {
    await manager.disposeAllAndWait();
  }
}

async function observeOwnerRelease() {
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
  const lifecycle = lifecycleRecorder();
  let starts = 0;
  const manager = new ESCodeAgentProcessManager({
    processLifecycleReporter: lifecycle.reporter,
    commandResolver: () => (starts++ === 0 ? retiring : replacement),
  });
  try {
    await manager.getClient(workspace);
    await waitUntil(() => manager.getExistingClient(workspace) === undefined);
    const client = await manager.getClient(workspace);
    await new Promise((r) => setTimeout(r, 400));
    return canonical({
      starts,
      replacementKeptRunning: manager.getExistingClient(workspace) === client,
      ownerPathReleased: !existsSync(owner),
      lifecycle: lifecycle.events,
    });
  } finally {
    await manager.disposeAllAndWait();
    await rm(dir, { recursive: true, force: true });
  }
}

test("a Rust runtime that exits before ready does not switch later starts to Node", async () => {
  const observed = await nodeOracle("crash-before-ready", observeCrashBeforeReady);
  // 两次启动都由 resolver 选择 Rust，且 resolver 从未收到 fallback 状态。
  assert.deepEqual(observed.resolverFallbackStates, [null, null]);
  assert.equal(observed.starts, 2);
  assert.deepEqual(observed.runtimes, ["escode-cli-rust", "escode-cli-rust"]);
  assert.equal(observed.secondClientReturned, true);
  // 冻结的失败形状：两代都未就绪即异常退出，退出码 3。
  assert.deepEqual(
    observed.lifecycle.filter((event) => event.type === "exit"),
    [
      {
        type: "exit",
        runtimeGeneration: 1,
        exitCode: 3,
        signal: null,
        terminationKind: "unexpected",
        runtimeReady: false,
        stderrLineCount: 0,
      },
      {
        type: "exit",
        runtimeGeneration: 2,
        exitCode: 3,
        signal: null,
        terminationKind: "unexpected",
        runtimeReady: false,
        stderrLineCount: 0,
      },
    ],
  );
});

test("a replacement runtime starts only after the retiring runtime released the workspace", async () => {
  const observed = await nodeOracle("owner-release", observeOwnerRelease);
  assert.equal(observed.starts, 2);
  assert.equal(
    observed.replacementKeptRunning,
    true,
    "the replacement keeps running instead of exiting on an owned workspace",
  );
  // 新一代启动时旧一代已删除 owner 标记，否则会以 75 退出。
  assert.equal(observed.ownerPathReleased, true);
  assert.deepEqual(
    observed.lifecycle.filter((event) => event.type === "exit").map((event) => event.exitCode),
    [0],
  );
});
