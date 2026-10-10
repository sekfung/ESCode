import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { MockBinding } from "@serialport/binding-mock";
import {
  SERIAL_WRITE_LIMIT_BYTES,
  SerialError,
  type SerialConfig,
  type SerialStatus,
} from "../src/serial/serial.js";
import {
  createSerialService,
  shouldRegisterSerialService,
  type SerialService,
} from "../src/serial/serialService.js";
import { mapSerialOpenError } from "../src/serial/serialErrors.js";

import { createTestBinding, TEST_SERIAL_CONFIG, waitFor } from "./serialTestBinding.js";

const CONFIG: SerialConfig = TEST_SERIAL_CONFIG;

let testBinding: ReturnType<typeof createTestBinding>;
let service: SerialService;
let statuses: SerialStatus[];

function create(overrides: Partial<Parameters<typeof createSerialService>[0]> = {}) {
  const created = createSerialService({
    loadBinding: async () => testBinding.binding,
    pollIntervalMs: 5,
    coalesceWindowMs: 0,
    ...overrides,
  });
  return created;
}

beforeEach(() => {
  MockBinding.reset();
  MockBinding.createPort("/dev/ttyMOCK0", { record: true });
  MockBinding.createPort("/dev/ttyMOCK1");
  testBinding = createTestBinding();
  service = create();
  statuses = [];
  service.onStatus((status) => statuses.push(status));
});

afterEach(async () => {
  await service.disposeAllAndWait();
});

test("list 返回 binding 枚举到的串口", async () => {
  const ports = await service.list();
  assert.deepEqual(
    ports.map((port) => port.path),
    ["/dev/ttyMOCK0", "/dev/ttyMOCK1"],
  );
});

test("打开后 RX/TX 进入快照，seq 单调递增并带方向与来源", async () => {
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  assert.equal(statuses.at(-1)?.state, "open");

  await service.write({
    bytes: new TextEncoder().encode("AT\r\n"),
    source: "user",
  });
  testBinding.emit("/dev/ttyMOCK0", "OK\r\n");
  await waitFor(async () => (await service.getSnapshot()).chunks.length === 2);

  const snapshot = await service.getSnapshot();
  assert.deepEqual(
    snapshot.chunks.map((chunk) => [
      chunk.seq,
      chunk.direction,
      chunk.source,
      Buffer.from(chunk.bytes).toString(),
    ]),
    [
      [1, "tx", "user", "AT\r\n"],
      [2, "rx", "user", "OK\r\n"],
    ],
  );
  assert.equal(snapshot.seq, 2);
  assert.deepEqual(snapshot.stats, { rxBytes: 4, txBytes: 4 });
  assert.equal(snapshot.status.state, "open");
  assert.equal(snapshot.status.path, "/dev/ttyMOCK0");
});

test("onData 推送的 chunk 与快照一致", async () => {
  const seen: number[] = [];
  service.onData((chunk) => seen.push(chunk.seq));
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  await service.write({ bytes: Uint8Array.of(1, 2), source: "agent" });
  testBinding.emit("/dev/ttyMOCK0", Buffer.from([3]));
  await waitFor(() => seen.length === 2);
  assert.deepEqual(seen, [1, 2]);
  const snapshot = await service.getSnapshot();
  assert.equal(snapshot.chunks[0]?.source, "agent");
});

test("非 open 状态写入返回 notOpen，不排队", async () => {
  await assert.rejects(
    service.write({ bytes: Uint8Array.of(1), source: "user" }),
    (error: unknown) => error instanceof SerialError && error.code === "notOpen",
  );
});

test("单次写入超过 64 KiB 返回 invalidInput", async () => {
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  await assert.rejects(
    service.write({
      bytes: new Uint8Array(SERIAL_WRITE_LIMIT_BYTES + 1),
      source: "user",
    }),
    (error: unknown) => error instanceof SerialError && error.code === "invalidInput",
  );
});

test("环形缓冲超过字节上限时淘汰最旧的完整 chunk", async () => {
  await service.disposeAllAndWait();
  service = create({ bufferLimitBytes: 6 });
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  for (const text of ["aaa", "bbb", "cc"]) {
    await service.write({
      bytes: new TextEncoder().encode(text),
      source: "user",
    });
  }
  const snapshot = await service.getSnapshot();
  assert.deepEqual(
    snapshot.chunks.map((chunk) => Buffer.from(chunk.bytes).toString()),
    ["bbb", "cc"],
  );
  assert.equal(snapshot.seq, 3);
  // 计数统计全部历史，不随淘汰减少
  assert.equal(snapshot.stats.txBytes, 8);
});

test("clear 清空缓冲与计数但保持串口打开", async () => {
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  await service.write({ bytes: Uint8Array.of(1), source: "user" });
  await service.clear();
  const snapshot = await service.getSnapshot();
  assert.deepEqual(snapshot.chunks, []);
  assert.deepEqual(snapshot.stats, { rxBytes: 0, txBytes: 0 });
  assert.equal(snapshot.status.state, "open");
  // seq 不回退，避免 renderer 把新数据误判为快照内旧数据
  await service.write({ bytes: Uint8Array.of(2), source: "user" });
  assert.equal((await service.getSnapshot()).seq, 2);
});

test("相邻 RX 在合并窗口内合成一个 chunk", async () => {
  await service.disposeAllAndWait();
  service = create({ coalesceWindowMs: 30 });
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  testBinding.emit("/dev/ttyMOCK0", "ab");
  await new Promise((resolve) => setTimeout(resolve, 2));
  testBinding.emit("/dev/ttyMOCK0", "cd");
  await waitFor(async () => (await service.getSnapshot()).chunks.length === 1);
  await new Promise((resolve) => setTimeout(resolve, 60));
  const snapshot = await service.getSnapshot();
  assert.deepEqual(
    snapshot.chunks.map((chunk) => Buffer.from(chunk.bytes).toString()),
    ["abcd"],
  );
});

test("并发 open 共享同一次进行中的打开", async () => {
  await Promise.all([
    service.open({ path: "/dev/ttyMOCK0", config: CONFIG }),
    service.open({ path: "/dev/ttyMOCK0", config: CONFIG }),
  ]);
  assert.deepEqual(
    statuses.map((status) => status.state),
    ["opening", "open"],
  );
});

test("已打开时 open 另一个串口会先关闭当前串口", async () => {
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  await service.open({ path: "/dev/ttyMOCK1", config: CONFIG });
  const snapshot = await service.getSnapshot();
  assert.equal(snapshot.status.path, "/dev/ttyMOCK1");
  assert.deepEqual(
    statuses.map((status) => status.state),
    ["opening", "open", "closing", "closed", "opening", "open"],
  );
});

test("open 与 close 按到达顺序串行，最终为 closed", async () => {
  const opening = service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  const closing = service.close();
  await Promise.all([opening, closing]);
  assert.equal((await service.getSnapshot()).status.state, "closed");
});

test("串口被占用时进入 error/busy", async () => {
  const other = create();
  try {
    await other.open({ path: "/dev/ttyMOCK0", config: CONFIG });
    await assert.rejects(
      service.open({ path: "/dev/ttyMOCK0", config: CONFIG }),
      (error: unknown) => error instanceof SerialError && error.code === "busy",
    );
    const status = (await service.getSnapshot()).status;
    assert.equal(status.state, "error");
    assert.equal(status.error?.code, "busy");
  } finally {
    await other.disposeAllAndWait();
  }
});

test("串口不存在时进入 error/notFound", async () => {
  await assert.rejects(
    service.open({ path: "/dev/ttyNONE", config: CONFIG }),
    (error: unknown) => error instanceof SerialError && error.code === "notFound",
  );
});

test("非法参数在调用 binding 前返回 invalidConfig", async () => {
  await assert.rejects(
    service.open({ path: "/dev/ttyMOCK0", config: { ...CONFIG, baudRate: 0 } }),
    (error: unknown) => error instanceof SerialError && error.code === "invalidConfig",
  );
});

test("拔出且开启自动重连：disconnected，重新插入后用原参数恢复 open", async () => {
  await service.open({
    path: "/dev/ttyMOCK0",
    config: { ...CONFIG, baudRate: 9600 },
  });
  testBinding.unplug("/dev/ttyMOCK0");
  await waitFor(() => statuses.at(-1)?.state === "disconnected");
  testBinding.replug("/dev/ttyMOCK0");
  await waitFor(() => statuses.at(-1)?.state === "open");
  const status = (await service.getSnapshot()).status;
  assert.equal(status.config?.baudRate, 9600);
  await service.write({ bytes: Uint8Array.of(7), source: "user" });
});

test("拔出且关闭自动重连：直接 closed", async () => {
  await service.open({
    path: "/dev/ttyMOCK0",
    config: { ...CONFIG, autoReconnect: false },
  });
  testBinding.unplug("/dev/ttyMOCK0");
  await waitFor(() => statuses.at(-1)?.state === "closed");
});

test("disconnected 时 close 停止等待与轮询", async () => {
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  testBinding.unplug("/dev/ttyMOCK0");
  await waitFor(() => statuses.at(-1)?.state === "disconnected");
  await service.close();
  assert.equal(statuses.at(-1)?.state, "closed");
  const calls = testBinding.listCalls;
  testBinding.replug("/dev/ttyMOCK0");
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(testBinding.listCalls, calls);
  assert.equal((await service.getSnapshot()).status.state, "closed");
});

test("仅在面板可见时轮询串口列表并推送变化", async () => {
  const pushes: string[][] = [];
  service.onPorts((ports) => pushes.push(ports.map((port) => port.path)));
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(testBinding.listCalls, 0);

  await service.setWatching({ watching: true });
  await waitFor(() => pushes.length === 1);
  testBinding.unplug("/dev/ttyMOCK1");
  await waitFor(() => pushes.length === 2);
  assert.deepEqual(pushes[1], ["/dev/ttyMOCK0"]);

  await service.setWatching({ watching: false });
  const calls = testBinding.listCalls;
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.ok(testBinding.listCalls <= calls + 1);
});

test("原生模块不可用时返回 nativeUnavailable，创建服务本身不失败", async () => {
  const unavailable = create({
    loadBinding: async () => {
      throw new Error("Cannot find module bindings.node");
    },
  });
  try {
    await assert.rejects(
      unavailable.list(),
      (error: unknown) => error instanceof SerialError && error.code === "nativeUnavailable",
    );
    await assert.rejects(
      unavailable.open({ path: "/dev/ttyMOCK0", config: CONFIG }),
      (error: unknown) => error instanceof SerialError && error.code === "nativeUnavailable",
    );
  } finally {
    await unavailable.disposeAllAndWait();
  }
});

test("dispose 释放串口，其他实例可以重新打开", async () => {
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
  await service.disposeAllAndWait();
  service = create();
  await service.open({ path: "/dev/ttyMOCK0", config: CONFIG });
});

test("平台错误消息映射为错误码", () => {
  const cases: Array<[string, string]> = [
    ["Opening COM3: Access denied", "busy"],
    ["Error: Resource busy, cannot open /dev/ttyUSB0", "busy"],
    ["Error Resource temporarily unavailable Cannot lock port", "busy"],
    ["Port is locked cannot open", "busy"],
    ["Error: Permission denied, cannot open /dev/ttyUSB0", "denied"],
    ["Opening COM9: File not found", "notFound"],
    ["Error: No such file or directory, cannot open /dev/ttyUSB9", "notFound"],
    ["Port does not exist - please call MockBinding.createPort('x') first", "notFound"],
    ["Error: Invalid argument, cannot open /dev/ttyUSB0", "invalidConfig"],
    ["something else", "io"],
  ];
  for (const [message, code] of cases) {
    assert.equal(mapSerialOpenError(new Error(message)).code, code, message);
  }
});

test("只有 Desktop Local Host 注册串口服务", () => {
  assert.equal(shouldRegisterSerialService("desktop-local"), true);
  assert.equal(shouldRegisterSerialService("desktop-attached-remote"), false);
  assert.equal(shouldRegisterSerialService("standalone-server"), false);
  assert.equal(shouldRegisterSerialService(undefined), false);
});

test("设置 ZCODE_SERIAL_MOCK_PORTS 时默认 binding 使用回环虚拟串口（仅供 E2E）", async () => {
  const { loadDefaultSerialBinding } = await import("../src/serial/serialService.js");
  const previous = process.env.ZCODE_SERIAL_MOCK_PORTS;
  process.env.ZCODE_SERIAL_MOCK_PORTS = "COM_MOCK_A, COM_MOCK_B";
  const mocked = createSerialService({
    loadBinding: loadDefaultSerialBinding,
    coalesceWindowMs: 0,
  });
  try {
    assert.deepEqual(
      (await mocked.list()).map((port) => port.path),
      ["COM_MOCK_A", "COM_MOCK_B"],
    );
    await mocked.open({ path: "COM_MOCK_A", config: CONFIG });
    await mocked.write({ bytes: new TextEncoder().encode("ping"), source: "user" });
    await waitFor(async () => (await mocked.getSnapshot()).chunks.length === 2);
    const snapshot = await mocked.getSnapshot();
    assert.deepEqual(
      snapshot.chunks.map((chunk) => [chunk.direction, Buffer.from(chunk.bytes).toString()]),
      [
        ["tx", "ping"],
        ["rx", "ping"],
      ],
    );
  } finally {
    await mocked.disposeAllAndWait();
    if (previous === undefined) delete process.env.ZCODE_SERIAL_MOCK_PORTS;
    else process.env.ZCODE_SERIAL_MOCK_PORTS = previous;
  }
});
