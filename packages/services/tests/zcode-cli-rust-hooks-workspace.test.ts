import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdir, writeFile } from "node:fs/promises";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-hooks.md H2：工作区（项目）hooks 的软门禁与审核。项目配置声明 UserPromptSubmit hook：首轮待信任
// （不执行，`workspaceHookAdmission` 提示条出现）；requestWorkspaceHookReview 打开审核交互，respondWorkspaceHookReview
// 信任全部项后提示条消失，下一轮项目 hook 执行并追加上下文。比对状态、审核载荷、ACK、模型请求与 hook 行。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

const hookScript = `
let raw = "";
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  const input = JSON.parse(raw);
  process.stdout.write(JSON.stringify({ hookSpecificOutput: { hookEventName: input.hookEventName, additionalContext: "project context" } }));
});
`;

async function observe(kind: "node" | "rust") {
  const requests: any[] = [];
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    event(res, { content: `answer ${requests.length}` });
    end(res, "stop");
  };
  const env = {
    ZCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle),
    ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath,
    ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle,
  };
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          env,
        })
      : await fixture({ registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    const script = join(f.root, "hook.cjs");
    await writeFile(script, hookScript);
    await mkdir(join(f.cwd, ".zcode"), { recursive: true });
    await writeFile(
      join(f.cwd, ".zcode", "config.json"),
      JSON.stringify({
        hooks: {
          enabled: true,
          events: {
            UserPromptSubmit: [{ hooks: [{ type: "process", command: process.execPath, args: [script] }] }],
          },
        },
      }),
    );
    const h: Harness = f.start();
    const sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    let after = h.messages.length;
    await h.command(h.envelope("sendText", sessionId, { text: "hello", mode: "yolo" }));
    await h.completed(sessionId, after);
    const states = () => {
      let admission: any;
      let review: any;
      for (const m of h.messages) {
        for (const delta of m.params?.frame?.payload?.deltas ?? []) {
          if (delta.patch && "workspaceHookAdmission" in delta.patch) admission = delta.patch.workspaceHookAdmission;
          if (delta.patch?.pendingInteractions) {
            review = delta.patch.pendingInteractions.find((p: any) => p.kind === "workspaceHookReview") ?? null;
          }
        }
      }
      return { admission, review };
    };
    const wait = async (check: () => boolean) => {
      const deadline = Date.now() + 15_000;
      while (!check() && Date.now() < deadline) await new Promise((done) => setTimeout(done, 50));
    };
    await wait(() => Boolean(states().admission));
    const pending = states().admission;
    const requestAck = await h.command(
      h.envelope("requestWorkspaceHookReview", sessionId, {
        sessionId,
        workspaceIdentity: pending?.workspaceIdentity ?? f.cwd,
        bundleDigest: pending?.bundleDigest,
      } as any),
    );
    await wait(() => Boolean(states().review));
    const review = states().review?.payload;
    const respondAck = review
      ? await h.command(
          h.envelope("respondWorkspaceHookReview", sessionId, {
            sessionId,
            taskId: review.taskId,
            runId: review.runId,
            workspaceIdentity: review.workspaceIdentity,
            bundleDigest: review.bundleDigest,
            reviewFlowId: review.reviewFlowId,
            generation: review.generation,
            interactionId: review.interactionId,
            decision: { action: "trust_selected", reviewItemIds: review.items.map((i: any) => i.reviewItemId) },
          } as any),
        )
      : undefined;
    await wait(() => states().admission === null && states().review === null);
    const settled = states();
    after = h.messages.length;
    await h.command(h.envelope("sendText", sessionId, { text: "again", mode: "yolo" }));
    await h.completed(sessionId, after);
    await new Promise((done) => setTimeout(done, 300));
    await h.close();
    const rows = new Map<number, any>();
    for (const m of h.messages) {
      for (const delta of m.params?.frame?.payload?.deltas ?? []) if (delta.row) rows.set(delta.row.rowId, delta.row);
    }
    return {
      pending: pending && { pendingCount: pending.pendingCount, digest: typeof pending.bundleDigest },
      requestAck: [requestAck.status, (requestAck as any).reasonCode],
      review: review && {
        keys: Object.keys(review).sort(),
        summary: review.summary,
        warningCode: review.warningCode,
        items: review.items.map((i: any) => ({ event: i.event, type: i.type, trustState: i.trustState, configuredEnabled: i.configuredEnabled })),
      },
      respondAck: respondAck && [respondAck.status, (respondAck as any).reasonCode],
      settled,
      contexts: requests.map((r) => r.messages.filter((m: any) => String(m.content).includes("project context")).length),
      hookRows: [...rows.values()]
        .filter((row) => row.kind === "hookInvocation")
        .map((row) => ({ hookEventName: row.hookEventName, state: row.state, sources: row.executions.map((e: any) => e.sourceKind) })),
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust gate and review workspace hooks the same way", async () => {
  const node = await observe("node");
  if (process.env.ZCODE_DUMP) console.log(JSON.stringify(node, null, 1));
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  assert.equal(node.pending?.pendingCount, 1);
  assert.deepEqual(node.contexts, [0, 1]);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});

// Settings 无会话授权（`workspace/hooks/trustGrant`）：digest 与当前工作区快照不符时两侧同样拒绝。
test("Node and Rust answer a stale workspace hook trust grant the same way", async () => {
  const grant = async (kind: "node" | "rust") => {
    const env = { ZCODE_PLUGIN_HOST_EXEC_PATH: process.execPath, ZCODE_PLUGIN_HOST_ENTRYPOINT: nodeBundle };
    const respond = (_req: any, res: any) => res.end();
    const f =
      kind === "node"
        ? await fixture({
            command: process.execPath,
            args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
            registry: true,
            respond,
            env,
          })
        : await fixture({ registry: true, respond, env });
    try {
      await configureRegistry(f, false);
      await mkdir(join(f.cwd, ".zcode"), { recursive: true });
      await writeFile(
        join(f.cwd, ".zcode", "config.json"),
        JSON.stringify({ hooks: { events: { Stop: [{ hooks: [{ type: "command", command: "echo hi" }] }] } } }),
      );
      const h: Harness = f.start();
      const result = await h.client.request(
        "workspace/hooks/trustGrant",
        {
          workspace: { workspacePath: h.workspace, workspaceKey: h.workspace },
          bundleDigest: "a".repeat(64),
          hookDeclarationDigest: "b".repeat(64),
        },
        { parse: (value: unknown) => value } as any,
      );
      await h.close();
      return result;
    } finally {
      await f.close();
    }
  };
  const node = await grant("node");
  assert.equal((node as any).accepted, false);
  assert.deepEqual(await grant("rust"), node);
});
