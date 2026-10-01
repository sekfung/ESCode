import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";
import { readModelTrajectory } from "../src/zcode-agent/modelTrajectory.js";

// docs/specs/rust-model-io.md：Rust 写 model-io 记录，App「模型调用轨迹」侧栏读出与 Node 相同的内容。
// 用 App 自己的 readModelTrajectory 读两侧文件，比对映射后的记录（剔除 id / 时间 / 耗时等非确定字段）。
type Runtime = "node" | "rust";
const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");
const text = (m: any) =>
  typeof m.content === "string" ? m.content : JSON.stringify(m.content);

async function observe(kind: Runtime, fullRetention = false) {
  const root = await mkdtemp(join(tmpdir(), `zcode-model-io-${kind}-`));
  const respond = (req: any, res: any) => {
    if (req.stream === false || req.stream === undefined) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          id: "t",
          object: "chat.completion",
          choices: [
            {
              index: 0,
              message: { role: "assistant", content: "Title" },
              finish_reason: "stop",
            },
          ],
          // 与 SSE 路径的 end() 同一组 usage：Node 的标题请求不走流式、Rust 走流式（已知传输差异，
          // 见 rust-model-io.md），两条路径返回相同数值才能比对记录本身。
          usage: { prompt_tokens: 10, completion_tokens: 4 },
        }),
      );
      return;
    }
    res.writeHead(200, { "content-type": "text/event-stream" });
    if (text(req.messages[0] ?? {}).startsWith("Generate a concise title")) {
      event(res, { content: "Title" });
      end(res, "stop");
      return;
    }
    const last = req.messages.at(-1);
    if (last.role === "user" && text(last).includes("list todos")) {
      event(res, { reasoning_content: "need the list" });
      event(res, {
        tool_calls: [
          {
            index: 0,
            id: "call-1",
            type: "function",
            function: { name: "TodoRead", arguments: "{}" },
          },
        ],
      });
      end(res, "tool_calls");
      return;
    }
    event(res, { content: last.role === "tool" ? "no todos" : "hello back" });
    end(res, "stop");
  };
  // 官方插件宿主环境与 App processManager 一致，否则 Rust 侧缺随包插件（技能 / node_repl）。
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
          args: ({ cwd }) => [
            nodeBundle,
            "app-server",
            "--stdio",
            "--cwd",
            cwd,
          ],
          registry: true,
          respond,
          titleRequests: "respond",
          mode: "yolo",
          env,
        })
      : await fixture({
          root,
          registry: true,
          respond,
          titleRequests: "respond",
          mode: "yolo",
          env,
        });
  let sessionId = "";
  let preference: unknown;
  try {
    await configureRegistry(f);
    const h = f.start();
    if (fullRetention) {
      const workspace = {
        workspacePath: h.workspace,
        workspaceKey: h.workspace,
      };
      preference = await h.client.request(
        "workspace/updateModelIoPreferences",
        { workspace, preferences: { fullRetentionEnabled: true } },
        { parse: (value: unknown) => value } as any,
      );
    }
    sessionId = await h.create();
    await h.subscribe(`conversation/${sessionId}`);
    let after = h.messages.length;
    await h.command(
      h.envelope("sendText", sessionId, { text: "list todos", mode: "yolo" }),
    );
    await h.completed(sessionId, after);
    after = h.messages.length;
    await h.command(
      h.envelope("sendText", sessionId, { text: "hi again", mode: "yolo" }),
    );
    await h.completed(sessionId, after);
    await h.close();
  } finally {
    await f.close();
  }
  // App 的读取按 os.homedir() 定位目录：临时指向该 runtime 的 root。
  const saved = {
    HOME: process.env.HOME,
    USERPROFILE: process.env.USERPROFILE,
  };
  process.env.HOME = root;
  process.env.USERPROFILE = root;
  let trajectory: Awaited<ReturnType<typeof readModelTrajectory>>;
  try {
    trajectory = await readModelTrajectory(sessionId);
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
  const rollout = join(root, ".zcode", "cli", "rollout");
  const files = await readdir(rollout).catch(() => []);
  // 落盘形态：bounded 模式有 delta/full 标记，全量保留模式不压缩、不带标记。
  const kinds: unknown[] = [];
  for (const name of files) {
    for (const line of (await readFile(join(rollout, name), "utf8")).split(
      "\n",
    )) {
      if (line.trim())
        kinds.push(JSON.parse(line).request?.messagesKind ?? null);
    }
  }
  await rm(root, { recursive: true, force: true });
  const scrub = (value: unknown) =>
    JSON.parse(
      JSON.stringify(value)
        .replaceAll(sessionId, "<session>")
        .replaceAll(JSON.stringify(root).slice(1, -1), "<root>"),
    );
  return {
    preference: scrub(preference ?? null),
    kinds,
    files: files.map((name) =>
      name.replace(/model-io-.*\.jsonl/, "model-io-<session>.jsonl"),
    ),
    records: trajectory.records.map((record) =>
      scrub({
        attempt: record.attempt,
        callSource: record.callSource,
        model: record.model,
        request: record.request,
        response: record.response && {
          finishReason: record.response.finishReason,
          text: record.response.text,
          reasoningText: record.response.reasoningText,
          toolCalls: record.response.toolCalls,
          usage: record.response.usage,
        },
        error: record.error,
      }),
    ),
  };
}

test("Rust writes model-io that the App trajectory pane reads like Node's", async () => {
  const node = await observe("node");
  const rust = await observe("rust");
  // 落盘的压缩形态允许不同：Node 的消息带随最新消息移动的 cache 标记，前缀指纹对不上，主回合总写 full；
  // Rust 投影后无这些标记，可以合法地写 delta。App 展开后的内容才是契约（下方逐条比对）。
  for (const kind of rust.kinds)
    assert.ok(["full", "delta", "tail"].includes(kind as string));
  assert.equal(rust.kinds.length, node.kinds.length);
  node.kinds = rust.kinds = [];
  // 先逐条比对，失败时更容易定位是哪一次调用。
  assert.equal(rust.records.length, node.records.length);
  node.records.forEach((record: unknown, index: number) =>
    assert.deepEqual(rust.records[index], record, `record ${index}`),
  );
  assert.deepEqual(rust, node);
  // 自检：两回合三次主调用（工具调用一次 + 工具结果后一次 + 第二回合一次），带推理与工具调用。
  const main = node.records.filter((r: any) => r.callSource.kind === "main");
  assert.equal(main.length, 3);
  assert.equal(main[0].response.reasoningText, "need the list");
  assert.equal(main[0].response.toolCalls[0].toolName, "TodoRead");
  assert.ok(main[2].request.messages.length > main[0].request.messages.length);
});

test("full model-IO retention is applied the same way as Node", async () => {
  const node = await observe("node", true);
  const rust = await observe("rust", true);
  assert.deepEqual(rust, node);
  assert.equal((node.preference as any).fullRetentionEnabled, true);
  assert.ok(
    node.kinds.length > 0 && node.kinds.every((kind: unknown) => kind === null),
  );
});
