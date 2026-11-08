import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { z } from "zod";
import { fixture, event, end, type Harness } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, goldenOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-legacy-session-methods.md「重导入与历史替换」：
// 同一 sessionId 重导入时删除旧导入段并保留真实续聊；Node 与 Rust 的消息序列/标题/时间一致。
const pluginHostBundle = resolve("apps/escode-cli/packages/cli/dist/plugin-host.cjs");
const any = z.any();
const PROVIDER = "personal:fixture";
const SESSION_ID = "claude-import-repair-1";
// 修复调用传入的是 Claude 源文件里的真实时间（早于用户后续续聊），Node 按 time_created 排序后
// 导入段自然落在续聊之前；这也是 Rust 固定「导入段 → 续聊」的前提。
const BASE = Date.UTC(2026, 0, 1);
const HOUR = 3_600_000;
type Runtime = "node" | "rust";

// 只比用户可见的对话：Node 的模型上下文提醒以隐藏 user 消息落库，Rust 的行投影不含它们。
const messages = (snapshot: any) =>
  snapshot.messages
    .filter(
      (message: any) =>
        message.info.synthetic !== true &&
        !["model-only", "hidden", "debug"].includes(message.info.visibility),
    )
    .map((message: any) => ({
      role: message.info.role,
      text: message.parts
        .filter((part: any) => part.type === "text")
        .map((part: any) => part.text)
        .join("\n"),
    }))
    .filter((message: any) => message.text.trim() !== "");

async function settle(h: Harness, sessionId: string, expected: string) {
  const started = Date.now();
  while (Date.now() - started < 15_000) {
    const snapshot: any = await h.client.request("session/read", { sessionId }, any);
    const userTexts = messages(snapshot)
      .filter((message: any) => message.role === "user")
      .map((message: any) => message.text);
    if (snapshot.session.status !== "running" && userTexts.at(-1) === expected) return snapshot;
    await delay(50);
  }
  throw new Error(`session ${sessionId} did not settle on ${expected}`);
}

async function observe(kind: Runtime) {
  const root = await mkdtemp(join(tmpdir(), `escode-import-repair-${kind}-`));
  const respond = async (_request: any, response: any) => {
    response.writeHead(200, { "content-type": "text/event-stream" });
    event(response, { content: "ack" });
    end(response, "stop");
  };
  const f =
    kind === "node"
      ? await fixture({
          root,
          command: process.execPath,
          args: ({ cwd }) => [pluginHostBundle, "app-server", "--stdio", "--cwd", cwd],
          registry: true,
          respond,
        })
      : await fixture({ root, registry: true, respond });
  const out: Record<string, unknown> = {};
  try {
    await configureRegistry(f);
    const h = f.start();
    const workspace = { workspacePath: f.cwd, workspaceKey: f.cwd };
    const createOn = (harness: Harness, params: Record<string, unknown>) =>
      harness.client.request("session/create", { workspace, ...params }, any) as Promise<any>;
    const create = (params: Record<string, unknown>) => createOn(h, params);
    const base = {
      sessionId: SESSION_ID,
      mode: "build",
      model: { providerId: PROVIDER, modelId: "model-a", options: { reasoningLevel: "low" } },
      thoughtLevel: "low",
      persistence: "immediate",
    };
    const history = (title: string, extra: boolean) => ({
      source: "claudeCode",
      title,
      createdAt: BASE,
      updatedAt: BASE + (extra ? 3 : 1) * HOUR,
      messages: [
        { role: "user", content: "q1", timestamp: BASE },
        { role: "assistant", content: "a1", timestamp: BASE + HOUR },
        ...(extra
          ? [
              { role: "user", content: "q2", timestamp: BASE + 2 * HOUR },
              { role: "assistant", content: "a2", timestamp: BASE + 3 * HOUR },
            ]
          : []),
      ],
    });
    // updatedAt 在 Node 是 runtime record 时间（写入时为 now，重启后回填库内时间），Rust 只有库内时间；
    // 本用例只比对导入替换的可见事实，时间戳差异记在 spec 的已知差异里。
    const facts = (snapshot: any) => ({
      createdAt: snapshot.session.createdAt,
      messages: messages(snapshot),
      title: snapshot.session.title,
      titleSource: snapshot.session.titleSource,
    });

    out.created = facts(await create({ ...base, importedHistory: history("Repair", false) }));
    // 导入后 Node 的 runtime 在 resume 时会清空模型选择（Host 打开任务时会重发 setModel），
    // 继续对话前两侧都显式设置同一模型。
    const setModel = () =>
      h.client.request("session/setModel", { sessionId: SESSION_ID, model: base.model }, any);
    await setModel();
    await h.client.request("session/send", { sessionId: SESSION_ID, content: "follow-up" }, any);
    out.followed = facts(await settle(h, SESSION_ID, "follow-up"));
    // 增长后的源历史：旧导入段整段替换，follow-up 仍排在后面。
    out.reimported = facts(
      await create({ ...base, importedHistory: history("Repair grown", true) }),
    );
    // 同一份历史重复导入：快照不变。
    out.repeated = facts(await create({ ...base, importedHistory: history("Repair grown", true) }));
    // 替换后仍可继续续聊。
    await setModel();
    await h.client.request("session/send", { sessionId: SESSION_ID, content: "after-repair" }, any);
    out.continued = facts(await settle(h, SESSION_ID, "after-repair"));
    await h.close();
    // 冷启动再从库里重导入：Rust 先 ensure_session 载入，Node 重建 record 后 resume。
    const again = f.start();
    out.coldReimport = facts(
      await createOn(again, { ...base, importedHistory: history("Repair grown", true) }),
    );
    await again.close();
    return out;
  } finally {
    await f.close();
  }
}

test("Node and Rust replace imported history on re-import and keep follow-up chat", async () => {
  const node = await goldenOracle("observe", () => observe("node"));
  const rust = await canonical(await observe("rust"));
  assert.deepEqual(rust, node);
});
