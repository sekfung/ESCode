import assert from "node:assert/strict";
import test from "node:test";
import { dirname, join, resolve } from "node:path";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { setTimeout as delay } from "node:timers/promises";
import { fixture, event, end, type Harness } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

// docs/specs/rust-v4-command-gaps.md「startSavedWorkflow」：设置页「已保存工作流」的「运行」在 Node 与
// Rust 上给出一致的命令 ACK、启动轮两行（turnHeader + userInput）、session/read 的标题与状态、
// 下一次模型请求里的启动句，以及 invalid_name / not_found / invalid_args / session_busy 四条拒绝
// （ACK 逐字一致、零行副作用）。
process.env.ZCODE_TEST_WAIT_MS ??= "60000";
type Message = Record<string, any>;
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
/** 无参档：脚本只做一次工作区读取，用于「成功启动」的成功路径。 */
const PLAIN_NAME = "count-txt";
/** 带必填实参的档：用于 invalid_args（缺失必填实参）。 */
const ARGS_NAME = "pr-review";
/** 忙判定用的输入：respond 见到它就不结束响应，把该轮按住在 running。 */
const HOLD_TEXT = "hold the turn";
const UUIDS = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;
const SAVED_WORKFLOW_HEAD = "/* zcode-workflow";
const WORKFLOW_BODY = [
  'const paths = await files.glob("*.txt");',
  "log(`found ${paths.length}`);",
  "return { count: paths.length };",
].join("\n");
const savedFile = (front: string[], script: string) => [...front, "*/", script, ""].join("\n");
const PLAIN_FILE = savedFile(
  [SAVED_WORKFLOW_HEAD, "description: Count the text files in the workspace."],
  WORKFLOW_BODY,
);
const ARGS_FILE = savedFile(
  [
    SAVED_WORKFLOW_HEAD,
    "description: Review the pull request end to end.",
    "args:",
    "  pr:",
    "    type: string",
    "    required: true",
    "    description: PR number or URL",
  ],
  "return { reviewed: true };",
);

const text = (content: unknown) =>
  typeof content === "string" ? content : JSON.stringify(content);
const until = async (condition: () => boolean, ms = 30_000) => {
  const deadline = Date.now() + ms;
  while (!condition() && Date.now() < deadline) await delay(50);
  return condition();
};

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-start-saved-workflow-${kind}-`));
  const requests: any[] = [];
  /** 忙判定用例里被挂住的模型响应；释放前该会话一直处于 running。 */
  let held: any;
  const respond = (req: any, res: any) => {
    requests.push(req);
    res.writeHead(200, { "content-type": "text/event-stream" });
    const last = req.messages?.at(-1);
    if (held === undefined && last?.role === "user" && text(last.content).includes(HOLD_TEXT)) {
      held = res;
      return;
    }
    event(res, { content: "noted" });
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
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
          mode: "yolo",
          env,
        })
      : await fixture({ root, registry: true, respond, mode: "yolo", env });
  try {
    await configureRegistry(f, false);
    const h: Harness = f.start();
    const parse = { parse: (value: unknown) => value } as any;
    await h.client.request(
      "workspace/updateDynamicWorkflowPolicy",
      { workspace: { workspacePath: h.workspace, workspaceKey: h.workspace }, enabled: true },
      parse,
    );
    const scoped = join(h.workspace, ".zcode", "workflows");
    await mkdir(scoped, { recursive: true });
    await writeFile(join(scoped, `${PLAIN_NAME}.dwf.ts`), PLAIN_FILE);
    await writeFile(join(scoped, `${ARGS_NAME}.dwf.ts`), ARGS_FILE);
    await writeFile(join(h.workspace, "notes.txt"), "notes");

    const ack = (sessionId: string, payload: Message) =>
      h.command(h.envelope("startSavedWorkflow", sessionId, payload));
    const projectRows = async (sessionId: string) => ((await h.rows(sessionId)).rows as any[]) ?? [];

    // (1) 成功启动：ACK + 启动轮两行 + session/read + 下一次模型请求里的启动句。
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    const launch: any = await ack(id, { name: PLAIN_NAME });
    const launchRows = await projectRows(id);
    const read: any = await h.client.request("session/read", { sessionId: id }, parse);
    const mark = h.messages.length;
    await h.command(h.envelope("sendText", id, { text: "next", mode: "yolo" }));
    await h.completed(id, mark);
    const asked = await until(() =>
      requests.some((request) =>
        (request.messages ?? []).some(
          (message: Message) =>
            message.role === "user" &&
            text(message.content).includes("Started the saved workflow"),
        ),
      ),
    );

    // (2) 忙会话拒绝：在跑的 turn 上启动必须零副作用（无 run、无消息、无事件、无任务）。
    const busyId = await h.create();
    await h.subscribe(`conversation/${busyId}`);
    const busyMark = h.messages.length;
    await h.command(h.envelope("sendText", busyId, { text: HOLD_TEXT, mode: "yolo" }));
    const busyStarted = Date.now();
    let holding = false;
    while (!holding && Date.now() - busyStarted < 20_000) {
      holding = (await projectRows(busyId)).some((row) => row.kind === "turnHeader");
      if (!holding) await delay(25);
    }
    const busy: any = await ack(busyId, { name: PLAIN_NAME });
    const busyRows = await projectRows(busyId);
    // 被按住的模型响应晚于轮首行到达：Node 与 Rust 的模型前步骤耗时不同（Rust 先落 turnHeader 再做
    // 记忆 / 技能等宿主往返），只采样一次会把响应永久挂住。这里等它真正到达再放行。
    const heldTurn = await until(() => held !== undefined, 30_000);
    if (held !== undefined) {
      event(held, { content: "released" });
      end(held, "stop");
      held = undefined;
    }
    await h.completed(busyId, busyMark);

    // (3) 三条零副作用拒绝：非法名、找不到、缺必填实参。每条都开一个新的草稿会话。
    const rejection = async (payload: Message) => {
      const sessionId = await h.create();
      const result: any = await ack(sessionId, payload);
      const rows = await projectRows(sessionId);
      return {
        status: result.status,
        reasonCode: result.reasonCode ?? null,
        message: result.message ?? null,
        rows: rows.length,
        launchRows: rows.filter((row) => row.origin === "workflowLaunch").length,
      };
    };
    const invalidName = await rejection({ name: "../escape" });
    const notFound = await rejection({ name: "no-such-workflow" });
    const invalidArgs = await rejection({ name: ARGS_NAME });

    await h.close();
    if (!asked) assert.ok(asked, `${kind}: the launch sentence never reached the model`);
    assert.ok(holding, `${kind}: the busy turn never started`);
    assert.ok(heldTurn, `${kind}: the busy turn never reached the model`);

    const scrub = (value: string) =>
      value
        .replaceAll(JSON.stringify(root).slice(1, -1), "<root>")
        .replaceAll(root, "<root>")
        .replace(UUIDS, "<uuid>")
        .replace(/dwf[a-z]*[-_][A-Za-z0-9_-]+/g, "<id>")
        .replace(/Today's date is [^.]*\./g, "Today's date is <date>.");
    const normalize = (value: unknown) => JSON.parse(scrub(JSON.stringify(value)));
    // row / entity 身份在两侧由不同来源铸造（Node 用持久 user messageId、Rust 用本次 turn id），
    // 无法跨进程比较；比较的是行的内容、归属与元数据（见 rust-fork-child 用例的同一取舍）。
    const projectRow = (row: any) =>
      normalize({
        rowId: row.rowId,
        kind: row.kind,
        origin: row.origin ?? null,
        executionKind: row.executionKind ?? null,
        state: row.state ?? null,
        text: row.text ?? null,
        hasClientId: row.clientId !== undefined,
        hasActiveMs: row.activeMs !== undefined,
        hasHistoryRoundCount: row.historyRoundCount !== undefined,
        hasEndedAt: row.endedAt !== undefined,
        sourceCommandIdPresent: typeof row.sourceCommandId === "string",
        rootSourceCommandIdSameAsSource: row.rootSourceCommandId === row.sourceCommandId,
        workflowLaunch: row.workflowLaunch ?? null,
      });
    const launchSentence = requests
      .flatMap((request) => request.messages ?? [])
      .filter(
        (message: Message) =>
          message.role === "user" &&
          text(message.content).includes("Started the saved workflow"),
      )
      .map((message: Message) => scrub(text(message.content)))[0];
    return {
      ack: {
        status: launch.status,
        reasonCode: launch.reasonCode ?? null,
        message: launch.message ?? null,
        result: normalize(launch.result),
      },
      launchRows: launchRows.map(projectRow),
      session: {
        title: read.session?.title ?? null,
        titleSource: read.session?.titleSource ?? null,
        status: read.session?.status ?? null,
        sessionKind: read.session?.sessionKind ?? null,
      },
      launchSentence,
      busy: {
        status: busy.status,
        reasonCode: busy.reasonCode ?? null,
        message: busy.message ?? null,
        launchRows: busyRows.filter((row) => row.origin === "workflowLaunch").length,
      },
      invalidName,
      notFound,
      invalidArgs,
      schemaErrors: h.schemaErrors,
    };
  } finally {
    await f.close();
  }
}

test("Node and Rust start a saved workflow from the workflows hub the same way", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  assert.deepEqual(node.schemaErrors, []);
  // 自检：确认 Node 侧真的走到了成功启动与四条拒绝，避免两侧同样为空也判等。
  assert.equal(node.ack.status, "accepted");
  assert.equal(node.ack.result.type, "startSavedWorkflow");
  assert.deepEqual(
    node.launchRows.map((row) => row.kind),
    ["turnHeader", "userInput"],
  );
  assert.match(node.launchRows[0].workflowLaunch?.runId ?? "", /^dwfrun-<uuid>$/);
  assert.match(
    node.launchRows[1].text ?? "",
    /^Started the saved workflow "count-txt" \(project\) from the workflows hub as run /,
  );
  assert.equal(node.session.titleSource, "first_input");
  assert.match(node.launchSentence ?? "", /^Started the saved workflow/);
  for (const [name, rejection] of [
    ["invalidName", node.invalidName],
    ["notFound", node.notFound],
    ["invalidArgs", node.invalidArgs],
  ] as const) {
    assert.equal(rejection.status, "failed", name);
    assert.equal(
      rejection.reasonCode,
      `fault.command.savedWorkflowStartRejected.${name === "invalidName" ? "invalid_name" : name === "notFound" ? "not_found" : "invalid_args"}`,
    );
    assert.equal(rejection.rows, 0, name);
  }
  assert.equal(node.busy.status, "failed");
  assert.equal(node.busy.reasonCode, "fault.command.savedWorkflowStartRejected.session_busy");
  assert.equal(node.busy.launchRows, 0);
  for (const key of Object.keys(node)) {
    assert.deepEqual((rust as any)[key], (node as any)[key], key);
  }
});
