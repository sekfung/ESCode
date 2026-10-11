import assert from "node:assert/strict";
import { test } from "node:test";
import type { SerialBrokerRequest } from "@escode/shared/serial";
import { SERIAL_TOOLS, handleSerialToolCall } from "../src/tools.js";

const META = {
  "com.escode/request-context": { session_id: "session-a", turn_id: "turn-1", runtime_scope: "main" },
};

function call(name: string, args: unknown, send = async () => ({ ok: true as const, result: {} })) {
  return handleSerialToolCall({ name, args, meta: META, send, signal: new AbortController().signal });
}

test("工具清单包含全部串口工具，并按读写声明 annotations", () => {
  const byName = new Map(SERIAL_TOOLS.map((tool) => [tool.name, tool]));
  assert.deepEqual([...byName.keys()].sort(), [
    "serial_close",
    "serial_list",
    "serial_open",
    "serial_read",
    "serial_set_signals",
    "serial_wait_for",
    "serial_write",
  ]);
  for (const name of ["serial_list", "serial_read", "serial_wait_for"]) {
    assert.equal(byName.get(name)?.annotations?.readOnlyHint, true, name);
  }
  for (const name of ["serial_open", "serial_write", "serial_close", "serial_set_signals"]) {
    assert.equal(byName.get(name)?.annotations?.destructiveHint, true, name);
  }
  assert.equal(byName.get("serial_write")?.inputSchema.type, "object");
});

test("调用转成 broker 请求，带会话上下文与默认参数", async () => {
  const sent: Array<Omit<SerialBrokerRequest, "id" | "token">> = [];
  const result = await call("serial_write", { data: "AT", lineEnding: "crlf" }, async (request) => {
    sent.push(request);
    return { ok: true, result: { bytes: 4, seq: 3 } };
  });
  assert.deepEqual(sent, [
    {
      op: "write",
      args: { data: "AT", encoding: "utf-8", lineEnding: "crlf" },
      sessionId: "session-a",
      turnId: "turn-1",
      runtimeScope: "main",
    },
  ]);
  assert.equal(result.isError, undefined);
  assert.deepEqual(JSON.parse(result.content[0]!.text), { bytes: 4, seq: 3 });
});

test("serial_read 把文本放在正文，元数据另起一行", async () => {
  const result = await call("serial_read", {}, async () => ({
    ok: true,
    result: {
      text: "OK\r\n",
      lastSeq: 9,
      truncated: false,
      evicted: false,
      status: { state: "open" },
    },
  }));
  assert.equal(result.content[0]!.text, "OK\r\n");
  assert.deepEqual(JSON.parse(result.content[1]!.text), {
    lastSeq: 9,
    truncated: false,
    evicted: false,
    status: { state: "open" },
  });
});

test("参数非法、缺少会话上下文、broker 失败都返回 isError 并带错误码", async () => {
  const invalid = await call("serial_read", { maxBytes: 0 });
  assert.equal(invalid.isError, true);
  assert.match(invalid.content[0]!.text, /^\[invalidInput\]/);

  const noSession = await handleSerialToolCall({
    name: "serial_list",
    args: {},
    meta: {},
    send: async () => ({ ok: true, result: {} }),
    signal: new AbortController().signal,
  });
  assert.match(noSession.content[0]!.text, /^\[unavailable\]/);

  const failed = await call("serial_write", { data: "AT" }, async () => ({
    ok: false,
    error: { code: "notOpen", message: "Serial port is not open" },
  }));
  assert.equal(failed.isError, true);
  assert.equal(failed.content[0]!.text, "[notOpen] Serial port is not open");
});

test("未知工具返回错误", async () => {
  const result = await call("serial_format", {});
  assert.equal(result.isError, true);
});
