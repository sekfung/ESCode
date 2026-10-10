import assert from "node:assert/strict";
import test from "node:test";
import {
  escapeSerialPreview,
  serialBrokerRequestSchema,
  serialToolArgsSchemas,
  zcodeSerialMethodParamsSchemas,
  zcodeSerialMethods,
  SERIAL_READ_MAX_BYTES,
  SERIAL_WAIT_MAX_TIMEOUT_MS,
} from "../src/serial/index.js";

const TOKEN = "a".repeat(64);
const BASE = {
  id: "00000000-0000-4000-8000-000000000000",
  token: TOKEN,
  runtimeScope: "main",
  sessionId: "session-a",
};

test("预览转义控制字符与非 ASCII 字节", () => {
  assert.equal(
    escapeSerialPreview(Uint8Array.from([0x41, 0x54, 0x0d, 0x0a, 0x00, 0xff, 0x5c, 0x09])),
    "AT\\r\\n\\x00\\xFF\\\\\\t",
  );
});

test("工具参数应用默认值并拒绝越界", () => {
  assert.deepEqual(serialToolArgsSchemas.read.parse({}), {
    direction: "rx",
    encoding: "utf-8",
    maxBytes: 8192,
  });
  assert.ok(!serialToolArgsSchemas.read.safeParse({ maxBytes: SERIAL_READ_MAX_BYTES + 1 }).success);
  assert.deepEqual(serialToolArgsSchemas.waitFor.parse({ pattern: "READY" }), {
    pattern: "READY",
    timeoutMs: 10000,
    encoding: "utf-8",
  });
  assert.ok(
    !serialToolArgsSchemas.waitFor.safeParse({
      pattern: "x",
      timeoutMs: SERIAL_WAIT_MAX_TIMEOUT_MS + 1,
    }).success,
  );
  assert.deepEqual(serialToolArgsSchemas.open.parse({ path: "COM3", baudRate: 9600 }), {
    path: "COM3",
    baudRate: 9600,
    dataBits: 8,
    parity: "none",
    stopBits: 1,
    rtscts: false,
  });
  assert.deepEqual(serialToolArgsSchemas.write.parse({ data: "AT" }), {
    data: "AT",
    encoding: "utf-8",
    lineEnding: "none",
  });
  assert.ok(!serialToolArgsSchemas.write.safeParse({ data: "AT", extra: 1 }).success);
});

test("broker 请求按 op 校验参数，并要求 token 与会话", () => {
  assert.ok(
    serialBrokerRequestSchema.safeParse({ ...BASE, op: "write", args: { data: "AT" } }).success,
  );
  assert.ok(!serialBrokerRequestSchema.safeParse({ ...BASE, op: "write", args: {} }).success);
  assert.ok(!serialBrokerRequestSchema.safeParse({ ...BASE, op: "format", args: {} }).success);
  assert.ok(
    !serialBrokerRequestSchema.safeParse({ ...BASE, token: "short", op: "list", args: {} }).success,
  );
});

test("协议为每个操作提供独立方法，参数携带会话与 workspace 身份并拒绝未知字段", () => {
  assert.deepEqual(Object.values(zcodeSerialMethods).sort(), [
    "interaction/serialClose",
    "interaction/serialList",
    "interaction/serialOpen",
    "interaction/serialRead",
    "interaction/serialWaitFor",
    "interaction/serialWrite",
  ]);
  const params = {
    requestId: "req-1",
    sessionId: "session-a",
    workspaceKey: "C:/work",
    workspacePath: "C:/work",
    args: { sinceSeq: 0 },
  };
  const parsed = zcodeSerialMethodParamsSchemas.read.parse(params);
  assert.equal(parsed.args.maxBytes, 8192);
  assert.ok(!zcodeSerialMethodParamsSchemas.read.safeParse({ ...params, extra: true }).success);
  assert.ok(
    !zcodeSerialMethodParamsSchemas.write.safeParse({ ...params, args: { sinceSeq: 0 } }).success,
  );
});

test("取消方法按目标 requestId 终止等待，参数严格校验", async () => {
  const { zcodeSerialCancelMethod, zcodeSerialCancelParamsSchema } =
    await import("../src/serial/index.js");
  assert.equal(zcodeSerialCancelMethod, "interaction/serialCancel");
  assert.ok(
    zcodeSerialCancelParamsSchema.safeParse({ sessionId: "s", targetRequestId: "req-1" }).success,
  );
  assert.ok(!zcodeSerialCancelParamsSchema.safeParse({ sessionId: "s" }).success);
});
