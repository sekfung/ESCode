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
    "interaction/serialSetSignals",
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

test("多串口：除 list/open 外的工具都接受可选 path", () => {
  for (const op of ["write", "read", "waitFor", "close"] as const) {
    const base = op === "write" ? { data: "x" } : op === "waitFor" ? { pattern: "x" } : {};
    assert.equal(serialToolArgsSchemas[op].parse({ ...base, path: "COM3" }).path, "COM3", op);
    assert.equal(serialToolArgsSchemas[op].parse(base).path, undefined, op);
  }
  assert.ok(!serialToolArgsSchemas.list.safeParse({ path: "COM3" }).success);
});

test("serial_set_signals：dtr/rts 与 pulse 互斥，至少给出一项", async () => {
  const { zcodeSerialMethodResultSchemas } = await import("../src/serial/index.js");
  const schema = serialToolArgsSchemas.setSignals;
  assert.ok(schema.safeParse({ dtr: true }).success);
  assert.ok(schema.safeParse({ rts: false, path: "COM3" }).success);
  assert.ok(schema.safeParse({ pulse: "esp32" }).success);
  assert.ok(!schema.safeParse({}).success);
  assert.ok(!schema.safeParse({ pulse: "esp32", dtr: true }).success);
  assert.ok(!schema.safeParse({ pulse: "stm32" }).success);
  assert.equal(zcodeSerialMethods.setSignals, "interaction/serialSetSignals");
  // 状态里携带 signals，结果 schema 必须接受（strict schema 遇未知字段会整条拒收）
  assert.ok(
    zcodeSerialMethodResultSchemas.close.safeParse({
      status: { state: "open", path: "COM3", signals: { dtr: true, rts: false } },
    }).success,
  );
  assert.ok(
    zcodeSerialMethodResultSchemas.setSignals.safeParse({ signals: { dtr: false, rts: true } })
      .success,
  );
});

test("状态携带循环发送进度时结果 schema 仍接受", async () => {
  const { zcodeSerialMethodResultSchemas } = await import("../src/serial/index.js");
  assert.ok(
    zcodeSerialMethodResultSchemas.close.safeParse({
      status: { state: "open", path: "COM3", loop: { intervalMs: 100, sent: 3 } },
    }).success,
  );
  assert.ok(
    zcodeSerialMethodResultSchemas.close.safeParse({
      status: { state: "open", path: "COM3", loop: { intervalMs: 100, count: 5, sent: 3 } },
    }).success,
  );
});
