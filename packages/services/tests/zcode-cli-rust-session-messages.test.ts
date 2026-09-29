import assert from "node:assert/strict";
import test from "node:test";
import { zcodeSessionMessagesResultSchema } from "@zcode/shared";
import { event, end, fixture } from "./zcode-cli-rust-fixture.js";

// docs/specs/rust-session-loading.md：`session/messages`（TS `readMessages`）曾让 Rust 回 -32601。
// 本用例只校验 Rust 侧：响应必须满足 App 用的 `zcodeSessionMessagesResultSchema`，并逐条符合 TS
// `readMessages` 的分页语义（`afterMessageId` 取其后的全部、找不到则退回全部；`limit` 取**最后** N 条）。
//
// 为什么不做 Node/Rust 逐条 deepEqual（证据见 spec 的已知差异）：
// - Node 返回的是 legacy AI SDK 形状（`info.id/sessionID`、part 带 `id/sessionID/messageID`），
//   过不了 App 自己的 `zcodeSessionMessagesResultSchema`（要求 `messageId/partId/sessionId`）；
// - Node 的列表还包含一条「活」的占位 assistant（只有 step-start），出现时机晚于 turn 完成，
//   同一场景两次读取的条数会不同。
// 该方法的 App 调用方（`readSessionMessages`）在当前 UI 里没有活跃消费者。
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

test("Rust session/messages follows the TS paging contract and the App schema", async () => {
  const f = await fixture({ mode: "yolo", respond });
  try {
    const h = f.start();
    const read = (sessionId: string, extra: Record<string, unknown> = {}) =>
      h.client.request(
        "session/messages",
        { sessionId, ...extra },
        zcodeSessionMessagesResultSchema,
      );
    // 未激活的会话：与 Node `requireSession` 同文案。
    await assert.rejects(read("sess_missing"), (error: any) =>
      /Session is not active/.test(String(error)),
    );
    const sid = await h.create();
    await h.subscribe(`conversation/${sid}`);
    for (const text of ["first", "second"]) {
      await h.command(h.envelope("sendText", sid, { text }));
      await h.completed(sid);
    }
    const all = (await read(sid)).messages;
    // 两轮各产生 user + assistant（assistant 带工具 part），Rust 的历史来自已提交的 row。
    assert.equal(all.length, 4, JSON.stringify(all.map((m) => m.info.role)));
    assert.deepEqual(
      all.map((m) => m.info.role),
      ["user", "assistant", "assistant", "user"],
    );
    const afterFirst = (await read(sid, { afterMessageId: all[0]?.info.messageId })).messages;
    assert.deepEqual(
      afterFirst.map((m) => m.info.messageId),
      all.slice(1).map((m) => m.info.messageId),
    );
    // limit 取最后 N 条（TS `slice(-limit)`）。
    const lastOnly = (await read(sid, { limit: 1 })).messages;
    assert.deepEqual(
      lastOnly.map((m) => m.info.messageId),
      [all.at(-1)?.info.messageId],
    );
    const two = (await read(sid, { limit: 2 })).messages;
    assert.deepEqual(
      two.map((m) => m.info.messageId),
      all.slice(-2).map((m) => m.info.messageId),
    );
    // afterMessageId 不存在时返回全部；limit 超过总数也返回全部。
    assert.equal((await read(sid, { afterMessageId: "msg_missing" })).messages.length, all.length);
    assert.equal((await read(sid, { limit: 999 })).messages.length, all.length);
    assert.deepEqual(h.schemaErrors, []);
    await h.close();
  } finally {
    await f.close();
  }
});
