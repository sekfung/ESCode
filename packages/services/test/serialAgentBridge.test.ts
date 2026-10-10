import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { MockBinding } from "@serialport/binding-mock";
import { escodeSerialMethodParamsSchemas, type SerialToolOp } from "@escode/shared/serial";
import { SerialError } from "../src/serial/serial.js";
import {
  createSerialAgentBridge,
  type SerialAgentBridge,
} from "../src/serial/serialAgentBridge.js";
import { createSerialService, type SerialService } from "../src/serial/serialService.js";
import { createTestBinding, TEST_SERIAL_CONFIG, waitFor } from "./serialTestBinding.js";

const PATH = "/dev/ttyBRIDGE0";
const OTHER = "/dev/ttyBRIDGE1";
let testBinding: ReturnType<typeof createTestBinding>;
let service: SerialService;
let bridge: SerialAgentBridge;
let remembered: Record<string, boolean>;

function params<Op extends SerialToolOp>(op: Op, args: Record<string, unknown>, extra = {}) {
  return escodeSerialMethodParamsSchemas[op].parse({
    requestId: `req-${Math.random()}`,
    sessionId: "session-a",
    workspaceKey: "C:/work",
    workspacePath: "C:/work",
    args,
    ...extra,
  });
}

const rejectsWith = (code: string) => (error: unknown) =>
  error instanceof SerialError && error.code === code;

beforeEach(() => {
  MockBinding.reset();
  MockBinding.createPort(PATH);
  MockBinding.createPort(OTHER);
  testBinding = createTestBinding();
  service = createSerialService({
    loadBinding: async () => testBinding.binding,
    pollIntervalMs: 5,
    coalesceWindowMs: 0,
  });
  remembered = {};
  bridge = createSerialAgentBridge({
    getSerialService: () => service,
    getRememberedAutoReconnect: async (path) => remembered[path],
  });
});

afterEach(async () => {
  await service.disposeAllAndWait();
});

test("Host 未提供串口服务时返回 unavailable", async () => {
  const none = createSerialAgentBridge({
    getSerialService: () => undefined,
    getRememberedAutoReconnect: async () => undefined,
  });
  await assert.rejects(none.handle("list", params("list", {})), rejectsWith("unavailable"));
});

test("远程 workspace 会话返回 unavailable", async () => {
  await assert.rejects(
    bridge.handle("list", params("list", {}, { remoteSessionId: "remote-1" })),
    rejectsWith("unavailable"),
  );
});

test("list 返回串口与当前状态", async () => {
  const result = await bridge.handle("list", params("list", {}));
  assert.deepEqual(
    result.ports.map((port) => port.path),
    [PATH, OTHER],
  );
  assert.deepEqual(result.sessions, []);
});

test("open 在关闭状态下打开，autoReconnect 继承用户对该串口的偏好", async () => {
  remembered[PATH] = false;
  const result = await bridge.handle("open", params("open", { path: PATH, baudRate: 9600 }));
  assert.equal(result.reused, false);
  assert.equal(result.status.state, "open");
  assert.equal(result.status.config?.autoReconnect, false);
  assert.equal(result.status.config?.baudRate, 9600);
});

test("open 只复用完全一致的串口与参数，否则返回 busy 且不抢占", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  const reused = await bridge.handle("open", params("open", { path: PATH, baudRate: 115200 }));
  assert.equal(reused.reused, true);
  await assert.rejects(
    bridge.handle("open", params("open", { path: PATH, baudRate: 9600 })),
    rejectsWith("busy"),
  );
  assert.equal((await service.getSnapshot({ path: PATH })).status.path, PATH);
});

test("write 按文本与行尾编码，记录 Agent 来源与会话", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  const result = await bridge.handle("write", params("write", { data: "AT", lineEnding: "crlf" }));
  assert.equal(result.bytes, 4);
  const [chunk] = (await service.getSnapshot({ path: PATH })).chunks;
  assert.equal(Buffer.from(chunk!.bytes).toString(), "AT\r\n");
  assert.equal(chunk!.source, "agent");
  assert.equal(chunk!.sessionId, "session-a");
  assert.equal(result.seq, chunk!.seq);
});

test("write 的 HEX 输入非法时返回 invalidInput；未打开时返回 notOpen", async () => {
  await assert.rejects(
    bridge.handle("write", params("write", { data: "41" })),
    rejectsWith("notOpen"),
  );
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  await assert.rejects(
    bridge.handle("write", params("write", { data: "4", encoding: "hex" })),
    rejectsWith("invalidInput"),
  );
});

test("read 按游标与方向返回文本；both 时每段带方向前缀；hex 时按字节显示", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  await service.write({ path: PATH, bytes: Buffer.from("AT\r\n"), source: "user" });
  testBinding.emit(PATH, "OK\r\n");
  await waitFor(async () => (await service.getSnapshot({ path: PATH })).chunks.length === 2);
  const rx = await bridge.handle("read", params("read", { sinceSeq: 0 }));
  assert.equal(rx.text, "OK\r\n");
  assert.equal(rx.lastSeq, 2);
  const both = await bridge.handle("read", params("read", { sinceSeq: 0, direction: "both" }));
  assert.equal(both.text, "TX AT\r\nRX OK\r\n");
  const hex = await bridge.handle("read", params("read", { sinceSeq: 0, encoding: "hex" }));
  assert.equal(hex.text, "4F 4B 0D 0A");
  const fresh = await bridge.handle("read", params("read", {}));
  assert.equal(fresh.text, "");
  assert.equal(fresh.lastSeq, 2);
});

test("waitFor 匹配后返回匹配文本、上下文与 seq", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  const waiting = bridge.handle("waitFor", params("waitFor", { pattern: "ver (\\d+)" }));
  testBinding.emit(PATH, "boot ok\r\n");
  // mock 可能把连续 emit 合并成一次 read；等第一段入缓冲后再发，确保匹配落在第二段
  await waitFor(async () => (await service.getSnapshot({ path: PATH })).chunks.length === 1);
  testBinding.emit(PATH, "ver 42\r\n");
  const result = await waiting;
  assert.equal(result.matched, true);
  if (result.matched) {
    assert.equal(result.match, "ver 42");
    assert.equal(result.context, "boot ok\r\n");
    assert.equal(result.seq, 2);
  }
});

test("waitFor 超时返回最近 RX 作为 tail；非法正则返回 invalidInput", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  testBinding.emit(PATH, "stuck here");
  await waitFor(async () => (await service.getSnapshot({ path: PATH })).chunks.length === 1);
  const result = await bridge.handle(
    "waitFor",
    params("waitFor", { pattern: "READY", timeoutMs: 30 }),
  );
  assert.deepEqual(result, { matched: false, reason: "timeout", tail: "stuck here", lastSeq: 1 });
  await assert.rejects(
    bridge.handle("waitFor", params("waitFor", { pattern: "(" })),
    rejectsWith("invalidInput"),
  );
});

test("cancel 按 requestId 终止同一会话的 waitFor，其他会话无法取消", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  const request = params("waitFor", { pattern: "never", timeoutMs: 5000 });
  const waiting = bridge.handle("waitFor", request);
  let settled = false;
  void waiting.then(() => {
    settled = true;
  });
  bridge.cancel({ sessionId: "session-b", targetRequestId: request.requestId });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(settled, false);
  bridge.cancel({ sessionId: "session-a", targetRequestId: request.requestId });
  const result = await waiting;
  assert.equal(result.matched, false);
  if (!result.matched) assert.equal(result.reason, "cancelled");
  // 幂等：目标已结束时再次取消不报错
  bridge.cancel({ sessionId: "session-a", targetRequestId: request.requestId });
});

test("close 关闭串口；已关闭时直接返回状态", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  const closed = await bridge.handle("close", params("close", {}));
  assert.equal(closed.status.state, "closed");
  const again = await bridge.handle("close", params("close", { path: PATH }));
  assert.equal(again.status.state, "closed");
  // 没有活动会话时省略 path 视为未打开
  await assert.rejects(bridge.handle("close", params("close", {})), rejectsWith("notOpen"));
});

// --- 多串口：省略 path 的三种情况 -------------------------------------------------

test("省略 path：恰好一个会话时使用它", async () => {
  await service.open({ path: OTHER, config: TEST_SERIAL_CONFIG });
  const written = await bridge.handle("write", params("write", { data: "A" }));
  assert.equal(written.bytes, 1);
  const [chunk] = (await service.getSnapshot({ path: OTHER })).chunks;
  assert.equal(Buffer.from(chunk!.bytes).toString(), "A");
  const listed = await bridge.handle("list", params("list", {}));
  assert.deepEqual(
    listed.sessions.map((session) => session.path),
    [OTHER],
  );
});

test("省略 path：没有会话时 read 返回空，write 返回 notOpen", async () => {
  const read = await bridge.handle("read", params("read", { sinceSeq: 0 }));
  assert.equal(read.text, "");
  await assert.rejects(
    bridge.handle("write", params("write", { data: "A" })),
    rejectsWith("notOpen"),
  );
});

test("省略 path：多个会话时返回 invalidInput 并列出路径；指定 path 则正常", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  await service.open({ path: OTHER, config: TEST_SERIAL_CONFIG });
  await assert.rejects(
    bridge.handle("write", params("write", { data: "A" })),
    (error: unknown) =>
      error instanceof SerialError &&
      error.code === "invalidInput" &&
      error.message.includes(PATH) &&
      error.message.includes(OTHER),
  );
  const written = await bridge.handle("write", params("write", { data: "A", path: OTHER }));
  assert.equal(written.bytes, 1);
  assert.deepEqual((await service.getSnapshot({ path: PATH })).chunks, []);
});

test("不抢占按路径生效：另一串口被占用不影响打开新串口", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  const opened = await bridge.handle("open", params("open", { path: OTHER, baudRate: 9600 }));
  assert.equal(opened.reused, false);
  assert.equal(opened.status.path, OTHER);
  assert.equal((await service.getSnapshot({ path: PATH })).status.state, "open");
});

// --- DTR/RTS ---------------------------------------------------------------------

test("setSignals 设置 DTR/RTS 并返回生效状态；省略 path 时使用唯一会话", async () => {
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  const result = await bridge.handle("setSignals", params("setSignals", { dtr: false }));
  assert.deepEqual(result, { signals: { dtr: false, rts: true } });
  const pulsed = await bridge.handle(
    "setSignals",
    params("setSignals", { pulse: "arduino", path: PATH }),
  );
  assert.deepEqual(pulsed, { signals: { dtr: false, rts: true } });
});

test("setSignals 在串口未打开时返回 notOpen", async () => {
  await assert.rejects(
    bridge.handle("setSignals", params("setSignals", { dtr: true, path: PATH })),
    rejectsWith("notOpen"),
  );
});
