import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { escodeSessionStateSnapshotSchema } from "@escode/shared";
import { event, end, fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";

// docs/specs/rust-session-loading.md：App 的 `session/resume`（TS `resumeSession` +
// `activateSessionForResume`）是任务列表/会话恢复的入口。Rust 之前没有该方法，App 会拿到
// -32601「Unsupported method」。这里比对冷/热恢复的 snapshot 与恢复后的可用性。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
type Runtime = "node" | "rust";

function respond(request: any, response: any) {
  response.writeHead(200, { "content-type": "text/event-stream" });
  if (request.messages.at(-1)?.role === "user") {
    event(response, {
      tool_calls: [
        {
          index: 0,
          id: "call-write",
          type: "function",
          function: {
            name: "Write",
            arguments: JSON.stringify({ file_path: "resume.txt", content: "written" }),
          },
        },
      ],
    });
    end(response, "tool_calls");
    return;
  }
  event(response, { content: "done" });
  end(response, "stop");
}

function start(kind: Runtime, root: string) {
  return kind === "node"
    ? fixture({
        root,
        command: process.execPath,
        args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
        registry: true,
        mode: "yolo",
        respond,
      })
    : fixture({ root, registry: true, mode: "yolo", respond });
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-session-resume-${kind}-`));
  try {
    // 阶段 1：跑一轮带工具调用的会话。
    const first = await start(kind, root);
    await configureRegistry(first);
    const h1 = first.start();
    const sid = await h1.create();
    await h1.subscribe(`conversation/${sid}`);
    await h1.command(h1.envelope("sendText", sid, { text: "write it" }));
    await h1.completed(sid);
    const warm = await h1.client.request(
      "session/resume",
      { workspace: { workspacePath: first.cwd, workspaceKey: first.cwd }, sessionId: sid },
      escodeSessionStateSnapshotSchema,
    );
    await h1.close();
    await first.close();

    // 阶段 2：新进程冷恢复同一会话；带上 create 时的工具面门禁。
    const second = await start(kind, root);
    await configureRegistry(second);
    const h2 = second.start();
    const cold = await h2.client.request(
      "session/resume",
      {
        workspace: { workspacePath: second.cwd, workspaceKey: second.cwd },
        sessionId: sid,
        thoughtLevel: "low",
        dynamicWorkflowEnabled: true,
        offPeakToolEnabled: false,
      },
      escodeSessionStateSnapshotSchema,
    );
    // 恢复后必须能直接继续对话（这就是 App 任务切换/手机恢复的链路）。
    await h2.subscribe(`conversation/${sid}`);
    await h2.command(h2.envelope("sendText", sid, { text: "again" }));
    await h2.completed(sid);
    const tools = (second.requests.at(-1)?.tools ?? []).map((t: any) => t.function.name);
    const rows = (await h2.rows(sid)).rows as any[];
    const observation = {
      warm: project(warm),
      cold: project(cold),
      resumedTurns: rows.filter((r) => r.kind === "userInput").length,
      // 恢复参数生效：带 dynamicWorkflowEnabled 的恢复不应丢掉工作流只读工具。
      listSavedWorkflowsVisible: tools.includes("ListSavedWorkflows"),
      schemaErrors: [...h1.schemaErrors, ...h2.schemaErrors],
    };
    await h2.close();
    await second.close();
    return observation;
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

/** 只比对外可见、跨 runtime 可比的字段（id/时间/revision 各自生成）。 */
function project(snapshot: any) {
  return {
    sessionKind: snapshot.session.sessionKind,
    status: snapshot.session.status,
    title: snapshot.session.title,
    mode: snapshot.settings.mode.current,
    projectionStatus: snapshot.projection.status,
    // `projection.mode` 单列比对：冷恢复时 Node 会给出 "build"（materialize 用
    // derivePersistedSessionMode 的 hint，取不到就回落默认），而 settings.mode.current 是
    // 会话真实模式 "yolo"；Rust 两处都按持久化模式。见 spec 的已知差异。
    messageRoles: snapshot.messages.map((m: any) => m.info.role),
    // AI SDK 的 step-start/step-finish 只有 Node 的流式转换会产（App 不按名字消费），
    // 属既有的消息 part 粒度差异，不在本用例范围。
    partKinds: snapshot.messages.flatMap((m: any) =>
      m.parts
        .map((p: any) => (p.type === "tool" ? "tool" : p.type))
        .filter((kind: string) => !kind.startsWith("step-")),
    ),
    slashCommands: (snapshot.slashCommands ?? []).map((c: any) => c.name).sort(),
  };
}

test("Node and Rust resume a session the same way (cold and warm)", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(rust.warm, node.warm);
  assert.deepEqual(rust.cold, node.cold);
  assert.deepEqual(rust.schemaErrors, []);
  assert.deepEqual(node.schemaErrors, []);
  // 自检：冷恢复拿到了完整历史，并且可以继续对话。
  assert.ok(node.cold.partKinds.includes("tool"));
  assert.equal(node.resumedTurns, 2);
  assert.equal(rust.resumedTurns, 2);
  // 恢复参数（dynamicWorkflowEnabled）在两侧都让工作流只读工具可见。
  assert.equal(node.listSavedWorkflowsVisible, true);
  assert.equal(rust.listSavedWorkflowsVisible, true);
});
