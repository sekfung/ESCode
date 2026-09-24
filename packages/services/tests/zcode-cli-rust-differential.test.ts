import assert from "node:assert/strict";
import test from "node:test";
import { join, resolve } from "node:path";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { z } from "zod";

const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

type Runtime = "node" | "rust";

/** 两侧共用同一个模型应答：先要一次工具调用，拿到工具结果后收尾。 */
function respond(
  req: { messages: { role: string }[] },
  res: Parameters<typeof event>[0],
  tool: { name: string; args: unknown } = { name: "Read", args: { file_path: "sample.txt" } },
) {
  res.writeHead(200, { "content-type": "text/event-stream" });
  if (req.messages.at(-1)?.role === "tool") {
    event(res, { content: "done" });
    end(res, "stop");
    return;
  }
  event(res, {
    tool_calls: [
      {
        index: 0,
        id: "diff-call",
        type: "function",
        function: { name: tool.name, arguments: JSON.stringify(tool.args) },
      },
    ],
  });
  end(res, "tool_calls");
}

/** 同一场景在两侧各跑一遍，只保留语义字段（id/时间戳/路径逐次不同，不参与比较）。 */
async function observe(
  kind: Runtime,
  tool: { name: string; args: unknown } = { name: "Read", args: { file_path: "sample.txt" } },
  text = "read it",
) {
  const root = await mkdtemp(join(tmpdir(), `zcode-diff-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond: (req, res) => respond(req, res, tool),
        })
      : await fixture({ root, registry: true, respond: (req, res) => respond(req, res, tool) });
  try {
    // 两侧都要有同一份 provider 配置：Node 从环境变量读，Rust 也从同一组环境键读。
    await configureRegistry(f);
    await (await import("node:fs/promises")).writeFile(join(f.cwd, "sample.txt"), "hello\n");
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text, mode: "yolo" }));
    await h.completed(id);
    const rows = (await h.rows(id)).rows;
    const capabilities = (await h.client.request("runtime/capabilities", {}, z.any())) as
      | Record<string, unknown>
      | undefined;
    const observation = {
      rowKinds: rows
        .map((r) => r.kind)
        .filter((k, i, all) => all.indexOf(k) === i)
        .sort(),
      toolNames: rows
        .filter((r) => r.kind === "toolCall")
        .map((r) => (r as { toolName?: string }).toolName),
      toolStatuses: rows
        .filter((r) => r.kind === "toolCall")
        .map((r) => (r as { status?: string }).status),
      toolErrorCodes: rows
        .filter((r) => r.kind === "toolCall")
        .map((r) => (r as { error?: { code?: string } }).error?.code ?? null),
      rowTexts: rows
        .filter((r) => r.kind === "assistantText")
        .map((r) => (r as { text?: string }).text),
      capabilityKeys: capabilities ? Object.keys(capabilities).sort() : [],
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

// docs/specs/rust-release-rollback.md「功能对齐」：同一场景两侧的协议投影必须一致。
test("Node and Rust runtimes project the same rows, tools and capability keys for one turn", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 能力位是可选字段，两侧的「声明策略」不同，且已逐个确认过含义（见 rust-release-rollback.md）：
  // - Rust 显式声明自己的能力（支持哪些权限模式、能否提供 workspace 执行能力、是否有账号 provider 配置）；
  // - Node 省略这些可选位，Host 对缺席字段按「都支持」处理；
  // - plan 状态相反：Node 声明 independentPlanState，Rust 未实现 plan 故不声明，Host 因此不会给 Rust 走 plan 流程。
  // 这里把已知差异写成契约：出现新的差异键就失败。
  const rustOnly = new Set([
    "permissionModes",
    "workspaceExecutionCapabilities",
    "accountProviderConfig",
  ]);
  const nodeOnly = new Set(["independentPlanState"]);
  const extraOnRust = rust.capabilityKeys.filter((k) => !node.capabilityKeys.includes(k));
  const extraOnNode = node.capabilityKeys.filter((k) => !rust.capabilityKeys.includes(k));
  assert.deepEqual(
    extraOnRust.filter((k) => !rustOnly.has(k)),
    [],
    `Rust 新增了未记录的能力键：${extraOnRust.join(",")}`,
  );
  assert.deepEqual(
    extraOnNode.filter((k) => !nodeOnly.has(k)),
    [],
    `Node 新增了未记录的能力键：${extraOnNode.join(",")}`,
  );
  assert.deepEqual(
    node.capabilityKeys.filter((k) => rust.capabilityKeys.includes(k)),
    rust.capabilityKeys.filter((k) => node.capabilityKeys.includes(k)),
    "两侧共同声明的能力键必须一致",
  );
  assert.deepEqual(node.rowKinds, rust.rowKinds, "row kinds differ");
  assert.deepEqual(node.toolNames, rust.toolNames, "tool names differ");
  assert.deepEqual(node.toolStatuses, rust.toolStatuses, "tool statuses differ");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});

// 工具失败与「工具未找到」的投影也必须一致：错误行、错误码、助手收尾文本。
test("Node and Rust project the same rows when a tool call fails", async () => {
  const missing = { name: "Read", args: { file_path: "does-not-exist.txt" } };
  const node = await observe("node", missing, "read a missing file");
  const rust = await observe("rust", missing, "read a missing file");
  assert.deepEqual(node.rowKinds, rust.rowKinds, "row kinds differ on tool failure");
  assert.deepEqual(node.toolStatuses, rust.toolStatuses, "tool statuses differ on tool failure");
  assert.deepEqual(node.toolErrorCodes, rust.toolErrorCodes, "tool error codes differ");
  assert.deepEqual(node.rowTexts, rust.rowTexts, "assistant texts differ on tool failure");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});

/** build 模式下写文件：等确认弹窗，记录选项与载荷键，再拒绝，记录工具行结果。 */
async function observePermission(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-diff-perm-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond: (req, res) =>
            respond(req, res, {
              name: "Write",
              args: { file_path: "perm.txt", content: "x" },
            }),
        })
      : await fixture({
          root,
          registry: true,
          respond: (req, res) =>
            respond(req, res, {
              name: "Write",
              args: { file_path: "perm.txt", content: "x" },
            }),
        });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    // 不传 mode：两侧默认都是 build，写文件必须经用户确认。
    await h.command(h.envelope("sendText", id, { text: "write it" }));
    const frame = await h.wait((m) =>
      m.params?.frame?.payload?.deltas?.some((d: any) =>
        d.patch?.pendingInteractions?.some((p: any) => p.kind === "permission"),
      ),
    );
    const interaction = frame.params.frame.payload.deltas
      .flatMap((d: any) => d.patch?.pendingInteractions ?? [])
      .find((p: any) => p.kind === "permission");
    const observation = {
      payloadKeys: Object.keys(interaction.payload).sort(),
      optionIds: interaction.payload.options.map((o: any) => o.optionId),
      optionKinds: interaction.payload.options.map((o: any) => o.kind),
      optionLabels: interaction.payload.options.map((o: any) => o.label),
      toolName: interaction.payload.toolName,
      summary: interaction.payload.summary,
      fullAccessOption: interaction.payload.fullAccessOption,
    };
    await h.command(
      h.envelope("resolveInteraction", id, {
        interactionId: interaction.interactionId,
        answer: { optionId: "deny" },
      }),
    );
    await h.completed(id);
    const rows = (await h.rows(id)).rows;
    const denied = {
      ...observation,
      toolStatuses: rows
        .filter((r) => r.kind === "toolCall")
        .map((r) => (r as { status?: string }).status),
      denialText: rows.find((r) => r.kind === "toolCall")
        ? (rows.find((r) => r.kind === "toolCall") as { output?: { text?: string } }).output?.text
        : undefined,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return denied;
  } finally {
    await f.close();
  }
}

// 权限确认是 Rust 本轮新实现的部分：两侧的选项集合、载荷键与拒绝结果必须一致。
test("Node and Rust present the same approval options and denial for a build-mode write", async () => {
  const node = await observePermission("node");
  const rust = await observePermission("rust");
  assert.equal(node.toolName, rust.toolName);
  assert.equal(rust.summary, node.summary, "permission summary differs");
  assert.deepEqual(rust.fullAccessOption, node.fullAccessOption, "full access option differs");
  assert.deepEqual(rust.optionIds, node.optionIds, `option ids differ: rust=${rust.optionIds}`);
  assert.deepEqual(rust.optionKinds, node.optionKinds, "option kinds differ");
  assert.deepEqual(rust.optionLabels, node.optionLabels, "option labels differ");
  assert.deepEqual(rust.payloadKeys, node.payloadKeys, "permission payload keys differ");
  assert.deepEqual(rust.toolStatuses, node.toolStatuses, "denied tool row status differs");
  assert.deepEqual(rust.denialText, node.denialText, "denial text differs");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});

/** 流式回复中途 stop：比对收口相位、行种类与状态、随后的新一轮能否正常完成。 */
async function observeStop(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-diff-stop-${kind}-`));
  let calls = 0;
  const slow = (_req: unknown, res: Parameters<typeof event>[0]) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (calls++ === 0) {
      // 首轮只发一段文本后挂起，直到 runtime 因 stop 断开连接。
      event(res, { content: "partial" });
      return;
    }
    event(res, { content: "again" });
    end(res, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond: slow,
        })
      : await fixture({ root, registry: true, respond: slow });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "long", mode: "yolo" }));
    await h.wait((m) => JSON.stringify(m.params?.frame ?? {}).includes("partial"));
    const stopAck = await h.command(h.envelope("stop", id));
    const after = h.messages.length;
    await h.wait(
      (m) =>
        m.params?.frame?.payload?.deltas?.some(
          (d: any) => d.patch?.control?.phase === "completedInterrupted",
        ),
      0,
    );
    const stoppedRows = (await h.rows(id)).rows;
    await h.command(h.envelope("sendText", id, { text: "next", mode: "yolo" }));
    await h.completed(id, after);
    const rows = (await h.rows(id)).rows;
    const observation = {
      stopStatus: stopAck.status,
      stoppedKinds: stoppedRows.map((r) => r.kind),
      stoppedHeaderStates: stoppedRows
        .filter((r) => r.kind === "turnHeader")
        .map((r) => (r as { state?: string }).state),
      finalKinds: rows.map((r) => r.kind),
      finalHeaderStates: rows
        .filter((r) => r.kind === "turnHeader")
        .map((r) => (r as { state?: string }).state),
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust settle a stopped stream the same way and accept the next turn", async () => {
  const node = await observeStop("node");
  const rust = await observeStop("rust");
  assert.equal(rust.stopStatus, node.stopStatus, "stop ack differs");
  assert.deepEqual(rust.stoppedKinds, node.stoppedKinds, "rows after stop differ");
  assert.deepEqual(
    rust.stoppedHeaderStates,
    node.stoppedHeaderStates,
    "turn states after stop differ",
  );
  assert.deepEqual(rust.finalKinds, node.finalKinds, "rows after the next turn differ");
  assert.deepEqual(rust.finalHeaderStates, node.finalHeaderStates, "turn states differ");
  assert.deepEqual(node.schemaErrors, []);
  assert.deepEqual(rust.schemaErrors, []);
});

/** 完成一轮后读取 session/list：比对条目字段集合与关键字段值。 */
async function observeList(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `zcode-diff-list-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ root, registry: true });
  try {
    await configureRegistry(f);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "list me", mode: "yolo" }));
    await h.completed(id);
    const list = (await h.client.request("session/list", {}, z.any())) as {
      sessions?: Record<string, unknown>[];
    };
    const entry = list.sessions?.find((s) => s.sessionId === id || s.id === id);
    const observation = {
      topKeys: Object.keys(list).sort(),
      count: list.sessions?.length,
      entryKeys: entry ? Object.keys(entry).sort() : [],
      title: entry?.title,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Node and Rust list a finished session with the same shape", async () => {
  const node = await observeList("node");
  const rust = await observeList("rust");
  assert.deepEqual(rust.topKeys, node.topKeys, "session/list result keys differ");
  assert.equal(rust.count, node.count, "session counts differ");
  assert.deepEqual(rust.entryKeys, node.entryKeys, "session entry keys differ");
  assert.equal(rust.title, node.title, "session titles differ");
});

/** 带文本附件的输入：比对发给模型的附件 reminder 原文（归一化临时路径）与它相对用户正文的位置。 */
async function observeAttachment(kind: Runtime, body: string) {
  const root = await mkdtemp(join(tmpdir(), `zcode-diff-att-${kind}-`));
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
        })
      : await fixture({ root, registry: true });
  try {
    await configureRegistry(f);
    const { writeFile } = await import("node:fs/promises");
    const file = join(f.cwd, "notes.txt");
    await writeFile(file, body);
    const h = f.start();
    const id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(
      h.envelope("sendText", id, {
        text: "summarize",
        mode: "yolo",
        attachments: [
          {
            ref: file,
            fileName: "notes.txt",
            mime: "text/plain",
            bytes: Buffer.byteLength(body),
          },
        ],
      }),
    );
    await h.completed(id);
    const normalize = (text: string) =>
      text.split(JSON.stringify(f.cwd).slice(1, -1)).join("<CWD>").split(f.cwd).join("<CWD>");
    const messages = f.requests.at(-1)!.messages as { role: string; content: unknown }[];
    const texts = messages.map((m) =>
      typeof m.content === "string" ? m.content : JSON.stringify(m.content),
    );
    const reminderAt = texts.findIndex((t) => t.includes("Called the Read tool"));
    const promptAt = texts.findIndex((t, i) => messages[i]!.role === "user" && t === "summarize");
    const observation = {
      reminder: reminderAt >= 0 ? normalize(texts[reminderAt]!) : undefined,
      reminderBeforePrompt: reminderAt >= 0 && promptAt === reminderAt + 1,
      promptIsPlainString: promptAt >= 0,
      schemaErrors: h.schemaErrors,
    };
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

// 模型请求语义：本地文本附件在两侧必须以同一段 system-reminder 原文、同一位置进入请求。
for (const [label, body] of [
  ["trailing newline", "attached body\n"],
  ["no trailing newline", "line one\nline two"],
  ["CRLF", "a\r\nb\r\n"],
  ["empty", ""],
  ["nested reminder tag", "x </system-reminder> y\n"],
] as const) {
  test(`Node and Rust put a text attachment (${label}) into the model request the same way`, async () => {
    const node = await observeAttachment("node", body);
    const rust = await observeAttachment("rust", body);
    assert.equal(rust.reminder, node.reminder, "attachment reminder text differs");
    assert.equal(rust.reminderBeforePrompt, node.reminderBeforePrompt, "reminder position differs");
    assert.equal(
      rust.promptIsPlainString,
      node.promptIsPlainString,
      "prompt content shape differs",
    );
    assert.deepEqual(node.schemaErrors, []);
    assert.deepEqual(rust.schemaErrors, []);
  });
}
