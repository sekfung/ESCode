import assert from "node:assert/strict";
import test from "node:test";
import { dirname, resolve } from "node:path";
import { z } from "zod";
import { escodeSessionMessagesResultSchema } from "@escode/shared";
import { event, end, fixture } from "./escode-cli-rust-fixture.js";
import { configureRegistry } from "./escode-cli-rust-registry-fixture.js";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-session-loading.md：`session/messages`（TS `readMessages`）曾让 Rust 回 -32601。
// Rust 响应必须满足 App 用的 `escodeSessionMessagesResultSchema`，并逐条符合 TS `readMessages`
// 的分页语义（`afterMessageId` 取其后的全部、找不到则退回全部；`limit` 取**最后** N 条）。
//
// 线面形状是已批准差异：Node 返回 legacy AI SDK 形状（`info.id/sessionID`、part 带
// `id/sessionID/messageID`），过不了 App schema；Rust 返回 App schema（`messageId/partId/sessionId`）。
// 差分用语义投影（去掉 synthetic/hidden reminder、step-start/step-finish 与未完成的占位 assistant），
// 对拍用户/模型可见的 role、文本与工具调用。
const nodeBundle = resolve("apps/escode-cli/packages/cli/dist/escode.cjs");
const rustPluginEnv = { ESCODE_OFFICIAL_PLUGINS_BASE_DIR: dirname(nodeBundle) };

function respond(request: any, response: any) {
  response.writeHead(200, { "content-type": "text/event-stream" });
  if (request.messages.at(-1)?.role === "user") {
    event(response, {
      tool_calls: [
        {
          index: 0,
          id: `call-${request.messages.length}`,
          type: "function",
          function: {
            name: "Write",
            arguments: JSON.stringify({ file_path: "messages.txt", content: "x" }),
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

/** 把两种线面投影成同一份语义事实（id/时间戳/step 边界不参与）。 */
function project(messages: any[]) {
  return (
    messages
      .filter(
        (message) =>
          !message.info?.synthetic && message.info?.semantics?.transcriptVisibility !== "hidden",
      )
      .map((message) => ({
        role: message.info?.role,
        parts: (message.parts ?? [])
          .filter((part: any) => part.type !== "step-start" && part.type !== "step-finish")
          .map((part: any) =>
            part.type === "tool"
              ? {
                  type: "tool",
                  tool: part.tool ?? part.toolName,
                  status: part.state?.status ?? part.status,
                  input: part.state?.input ?? part.input,
                  output: part.state?.output ?? part.output,
                }
              : { type: part.type, text: part.text },
          ),
      }))
      // 只含 step 边界的 assistant 是运行中的占位消息，读取时机不同不构成差异。
      .filter((message) => message.parts.length > 0)
  );
}

async function observe(kind: "node" | "rust") {
  const f =
    kind === "node"
      ? await fixture({
          command: process.execPath,
          args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
          mode: "yolo",
          registry: true,
          respond,
        })
      : await fixture({ env: rustPluginEnv, mode: "yolo", registry: true, respond });
  try {
    await configureRegistry(f);
    const h = f.start();
    const schema = kind === "rust" ? escodeSessionMessagesResultSchema : z.any();
    const read = (sessionId: string, extra: Record<string, unknown> = {}) =>
      h.client.request("session/messages", { sessionId, ...extra }, schema) as Promise<any>;
    // 未激活的会话：与 Node `requireSession` 同文案。
    await assert.rejects(read("sess_missing"), (error: any) =>
      /Session is not active/.test(String(error)),
    );
    const sid = await h.create();
    await h.subscribe(`conversation/${sid}`);
    for (const text of ["first", "second"]) {
      const at = h.messages.length;
      await h.command(h.envelope("sendText", sid, { text }));
      await h.completed(sid, at);
    }
    const all = (await read(sid)).messages as any[];
    const observation = { projected: canonical(project(all)), schemaErrors: h.schemaErrors };
    if (kind === "rust") {
      // 两轮各产生 user + assistant（第一轮 assistant 拆成工具与正文两条，见 rust-row-projection.md）。
      assert.deepEqual(
        all.map((m) => m.info.role),
        ["user", "assistant", "assistant", "user", "assistant", "assistant"],
      );
      assert.deepEqual(
        all.map((m) => m.info.messageId).length,
        new Set(all.map((m) => m.info.messageId)).size,
      );
      const afterFirst = (await read(sid, { afterMessageId: all[0]?.info.messageId })).messages;
      assert.deepEqual(
        afterFirst.map((m: any) => m.info.messageId),
        all.slice(1).map((m) => m.info.messageId),
      );
      // limit 取最后 N 条（TS `slice(-limit)`）。
      const lastOnly = (await read(sid, { limit: 1 })).messages;
      assert.deepEqual(
        lastOnly.map((m: any) => m.info.messageId),
        [all.at(-1)?.info.messageId],
      );
      const two = (await read(sid, { limit: 2 })).messages;
      assert.deepEqual(
        two.map((m: any) => m.info.messageId),
        all.slice(-2).map((m) => m.info.messageId),
      );
      // afterMessageId 不存在时返回全部；limit 超过总数也返回全部。
      assert.equal(
        (await read(sid, { afterMessageId: "msg_missing" })).messages.length,
        all.length,
      );
      assert.equal((await read(sid, { limit: 999 })).messages.length, all.length);
      assert.deepEqual(h.schemaErrors, []);
    }
    await h.close();
    return observation;
  } finally {
    await f.close();
  }
}

test("Rust session/messages matches the TS semantic history and the App schema", async () => {
  const node = await nodeOracle("semantic", () => observe("node"));
  const rust = canonical(await observe("rust"));
  assert.deepEqual(rust.schemaErrors, []);
  assert.deepEqual(rust.projected, node.projected);
  assert.ok(node.projected.length > 0);
});
