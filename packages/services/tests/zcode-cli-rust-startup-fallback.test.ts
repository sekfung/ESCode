import assert from "node:assert/strict";
import test from "node:test";
import { tmpdir } from "node:os";
import {
  ZCodeAgentProcessManager,
  type ZCodeAgentCommand,
  type ZCodeAgentCommandResolverContext,
} from "../src/zcode-agent/zcodeAgentProcessManager.js";

// docs/specs/rust-packaging.md「启动失败自动回退」：Rust 进程在就绪前失败后，manager 让后续解析走 Node。
const workspace = { workspacePath: tmpdir() };
const crashingRust: ZCodeAgentCommand = {
  runtime: "zcode-cli-rust",
  command: process.execPath,
  args: ["-e", "process.exit(3)"],
};
const idleNode: ZCodeAgentCommand = {
  command: process.execPath,
  // 与真实 agent 一样在 stdin EOF 时退出，否则 dispose 会一直等待优雅退出。
  args: ["-e", 'process.stdin.resume(); process.stdin.on("end", () => process.exit(0));'],
};

async function waitUntil(predicate: () => boolean, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("condition not reached");
    await new Promise((r) => setTimeout(r, 25));
  }
}

test("a Rust runtime that exits before ready makes the next start resolve with rustRuntimeFailed", async () => {
  const contexts: ZCodeAgentCommandResolverContext[] = [];
  const manager = new ZCodeAgentProcessManager({
    commandResolver: (context) => {
      contexts.push(context);
      return context.rustRuntimeFailed ? idleNode : crashingRust;
    },
  });
  try {
    await manager.getClient(workspace);
    await waitUntil(() => manager.getExistingClient(workspace) === undefined);
    await manager.getClient(workspace);
    assert.equal(contexts.length, 2);
    assert.equal(contexts[0]!.rustRuntimeFailed, undefined, "first start tries Rust");
    assert.equal(contexts[1]!.rustRuntimeFailed, true, "second start is told Rust failed");
    assert.equal((await manager.canStart(workspace)).available, true);
    assert.equal(contexts.at(-1)!.rustRuntimeFailed, true, "the fact persists for later starts");
  } finally {
    await manager.disposeAllAndWait();
  }
});

test("a Rust runtime that crashes after becoming ready does not trigger the fallback", async () => {
  const contexts: ZCodeAgentCommandResolverContext[] = [];
  const lateCrash: ZCodeAgentCommand = {
    runtime: "zcode-cli-rust",
    command: process.execPath,
    args: ["-e", "setTimeout(() => process.exit(4), 300)"],
  };
  const manager = new ZCodeAgentProcessManager({
    commandResolver: (context) => {
      contexts.push(context);
      return lateCrash;
    },
  });
  try {
    const client = await manager.getClient(workspace);
    manager.markReady(workspace, client);
    await waitUntil(() => manager.getExistingClient(workspace) === undefined);
    await manager.getClient(workspace);
    assert.equal(contexts[1]!.rustRuntimeFailed, undefined, "runtime faults after ready keep Rust");
  } finally {
    await manager.disposeAllAndWait();
  }
});
