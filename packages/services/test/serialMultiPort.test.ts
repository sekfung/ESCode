import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { MockBinding } from "@serialport/binding-mock";
import { SerialError } from "../src/serial/serial.js";
import { createSerialService, type SerialService } from "../src/serial/serialService.js";
import { createTestBinding, TEST_SERIAL_CONFIG, waitFor } from "./serialTestBinding.js";

const PORTS = ["/dev/ttyM0", "/dev/ttyM1", "/dev/ttyM2", "/dev/ttyM3", "/dev/ttyM4"];
let testBinding: ReturnType<typeof createTestBinding>;
let service: SerialService;

beforeEach(() => {
  MockBinding.reset();
  for (const path of PORTS) MockBinding.createPort(path);
  testBinding = createTestBinding();
  service = createSerialService({
    loadBinding: async () => testBinding.binding,
    pollIntervalMs: 5,
    coalesceWindowMs: 0,
  });
});

afterEach(async () => {
  await service.disposeAllAndWait();
});

const text = (bytes: Uint8Array) => Buffer.from(bytes).toString();

test("两个串口各自收发，数据与事件按 path 区分、互不串扰", async () => {
  const seen: Array<[string, string]> = [];
  service.onData((chunk) => seen.push([chunk.path, text(chunk.bytes)]));
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  await service.open({ path: PORTS[1]!, config: TEST_SERIAL_CONFIG });
  await service.write({ path: PORTS[0]!, bytes: Buffer.from("a"), source: "user" });
  testBinding.emit(PORTS[1]!, "b");
  await waitFor(() => seen.length === 2);
  assert.deepEqual(seen, [
    [PORTS[0], "a"],
    [PORTS[1], "b"],
  ]);
  const first = await service.getSnapshot({ path: PORTS[0]! });
  const second = await service.getSnapshot({ path: PORTS[1]! });
  assert.deepEqual(
    first.chunks.map((chunk) => text(chunk.bytes)),
    ["a"],
  );
  assert.deepEqual(
    second.chunks.map((chunk) => text(chunk.bytes)),
    ["b"],
  );
  assert.deepEqual(first.stats, { rxBytes: 0, txBytes: 1 });
});

test("最多同时 4 个活动会话；关闭一个后可以再打开", async () => {
  for (const path of PORTS.slice(0, 4)) await service.open({ path, config: TEST_SERIAL_CONFIG });
  await assert.rejects(
    service.open({ path: PORTS[4]!, config: TEST_SERIAL_CONFIG }),
    (error: unknown) => error instanceof SerialError && error.code === "invalidInput",
  );
  // 已打开的串口用相同参数再次 open 不占新名额
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  await service.close({ path: PORTS[0]! });
  await service.open({ path: PORTS[4]!, config: TEST_SERIAL_CONFIG });
  assert.deepEqual(
    (await service.listSessions()).map((session) => session.path).sort(),
    PORTS.slice(1).sort(),
  );
});

test("listSessions 不含已关闭会话；关闭后快照仍保留历史", async () => {
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  await service.write({ path: PORTS[0]!, bytes: Buffer.from("hi"), source: "user" });
  await service.close({ path: PORTS[0]! });
  assert.deepEqual(await service.listSessions(), []);
  const snapshot = await service.getSnapshot({ path: PORTS[0]! });
  assert.equal(snapshot.status.state, "closed");
  assert.deepEqual(
    snapshot.chunks.map((chunk) => text(chunk.bytes)),
    ["hi"],
  );
});

test("同一串口关闭后重开，seq 继续递增不回退", async () => {
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  const first = await service.write({ path: PORTS[0]!, bytes: Buffer.from("1"), source: "user" });
  await service.close({ path: PORTS[0]! });
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  const second = await service.write({ path: PORTS[0]!, bytes: Buffer.from("2"), source: "user" });
  assert.ok(second.seq > first.seq);
});

test("未打开过的路径：快照为 closed 空快照，写入返回 notOpen", async () => {
  const snapshot = await service.getSnapshot({ path: PORTS[3]! });
  assert.equal(snapshot.status.state, "closed");
  assert.deepEqual(snapshot.chunks, []);
  await assert.rejects(
    service.write({ path: PORTS[3]!, bytes: Buffer.from("x"), source: "user" }),
    (error: unknown) => error instanceof SerialError && error.code === "notOpen",
  );
});

test("一次轮询为所有断开的会话重连", async () => {
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  await service.open({ path: PORTS[1]!, config: TEST_SERIAL_CONFIG });
  testBinding.unplug(PORTS[0]!);
  testBinding.unplug(PORTS[1]!);
  const state = async (path: string) => (await service.getSnapshot({ path })).status.state;
  await waitFor(
    async () =>
      (await state(PORTS[0]!)) === "disconnected" && (await state(PORTS[1]!)) === "disconnected",
  );
  testBinding.replug(PORTS[0]!);
  testBinding.replug(PORTS[1]!);
  await waitFor(
    async () => (await state(PORTS[0]!)) === "open" && (await state(PORTS[1]!)) === "open",
  );
});

test("dispose 关闭所有会话", async () => {
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  await service.open({ path: PORTS[1]!, config: TEST_SERIAL_CONFIG });
  await service.disposeAllAndWait();
  service = createSerialService({
    loadBinding: async () => testBinding.binding,
    coalesceWindowMs: 0,
  });
  await service.open({ path: PORTS[0]!, config: TEST_SERIAL_CONFIG });
  await service.open({ path: PORTS[1]!, config: TEST_SERIAL_CONFIG });
});
