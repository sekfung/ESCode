import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { MockBinding } from "@serialport/binding-mock";
import type { SerialChunk } from "../src/serial/serial.js";
import { createSerialService, type SerialService } from "../src/serial/serialService.js";
import { createTestBinding, TEST_SERIAL_CONFIG, waitFor } from "./serialTestBinding.js";

const PATH = "/dev/ttyAGENT0";
let testBinding: ReturnType<typeof createTestBinding>;
let service: SerialService;

const text = (chunks: readonly SerialChunk[]) =>
  chunks.map((chunk) => Buffer.from(chunk.bytes).toString()).join("");

beforeEach(async () => {
  MockBinding.reset();
  MockBinding.createPort(PATH);
  testBinding = createTestBinding();
  service = createSerialService({
    loadBinding: async () => testBinding.binding,
    pollIntervalMs: 5,
    coalesceWindowMs: 0,
  });
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
});

afterEach(async () => {
  await service.disposeAllAndWait();
});

test("Agent 写入的 chunk 带 source=agent 与 sessionId", async () => {
  const written = await service.write({
    bytes: Uint8Array.of(1),
    source: "agent",
    sessionId: "session-a",
  });
  assert.deepEqual(written, { seq: 1 });
  const [chunk] = (await service.getSnapshot()).chunks;
  assert.equal(chunk?.source, "agent");
  assert.equal(chunk?.sessionId, "session-a");
});

test("readSince 未给游标时只返回当前 lastSeq，不返回历史", async () => {
  await service.write({ bytes: Buffer.from("old"), source: "user" });
  const result = service.readSince({ direction: "both", maxBytes: 1024 });
  assert.deepEqual(result.chunks, []);
  assert.equal(result.lastSeq, 1);
  assert.equal(result.truncated, false);
  assert.equal(result.evicted, false);
});

test("readSince 按游标增量读取并按方向过滤，lastSeq 越过被过滤的 chunk", async () => {
  await service.write({ bytes: Buffer.from("AT"), source: "user" });
  testBinding.emit(PATH, "OK");
  await waitFor(async () => (await service.getSnapshot()).chunks.length === 2);
  const rx = service.readSince({ sinceSeq: 0, direction: "rx", maxBytes: 1024 });
  assert.equal(text(rx.chunks), "OK");
  assert.equal(rx.lastSeq, 2);
  const both = service.readSince({ sinceSeq: 0, direction: "both", maxBytes: 1024 });
  assert.equal(text(both.chunks), "ATOK");
  assert.deepEqual(
    service.readSince({ sinceSeq: 2, direction: "both", maxBytes: 1024 }).chunks,
    [],
  );
});

test("readSince 超过 maxBytes 时截断，lastSeq 指向最后返回的 chunk", async () => {
  for (const part of ["aaaa", "bbbb", "cccc"]) {
    await service.write({ bytes: Buffer.from(part), source: "user" });
  }
  const first = service.readSince({ sinceSeq: 0, direction: "tx", maxBytes: 6 });
  assert.equal(text(first.chunks), "aaaa");
  assert.equal(first.truncated, true);
  assert.equal(first.lastSeq, 1);
  // 单个 chunk 超过上限时返回其前缀，剩余部分丢弃，避免游标原地踏步
  const tiny = service.readSince({ sinceSeq: 1, direction: "tx", maxBytes: 2 });
  assert.equal(text(tiny.chunks), "bb");
  assert.equal(tiny.truncated, true);
  assert.equal(tiny.lastSeq, 2);
});

test("readSince 在游标之后的数据已被清空或淘汰时标记 evicted", async () => {
  await service.write({ bytes: Buffer.from("a"), source: "user" });
  await service.clear();
  await service.write({ bytes: Buffer.from("b"), source: "user" });
  const result = service.readSince({ sinceSeq: 0, direction: "both", maxBytes: 1024 });
  assert.equal(text(result.chunks), "b");
  assert.equal(result.evicted, true);
});

test("waitFor 在新的 RX 满足条件时返回匹配结果", async () => {
  const waiting = service.waitFor({
    timeoutMs: 2000,
    test: (chunks) => (text(chunks).includes("READY") ? "found" : null),
  });
  testBinding.emit(PATH, "boot...");
  testBinding.emit(PATH, "READY\r\n");
  const result = await waiting;
  assert.equal(result.kind, "matched");
  assert.equal(result.kind === "matched" ? result.value : null, "found");
});

test("waitFor 默认忽略调用前的数据，传 sinceSeq 可匹配历史", async () => {
  testBinding.emit(PATH, "READY");
  await waitFor(async () => (await service.getSnapshot()).chunks.length === 1);
  const fresh = await service.waitFor({
    timeoutMs: 50,
    test: (chunks) => (text(chunks).includes("READY") ? true : null),
  });
  assert.equal(fresh.kind, "timeout");
  const history = await service.waitFor({
    sinceSeq: 0,
    timeoutMs: 50,
    test: (chunks) => (text(chunks).includes("READY") ? true : null),
  });
  assert.equal(history.kind, "matched");
});

test("waitFor 超时返回 timeout 与 lastSeq", async () => {
  const result = await service.waitFor({ timeoutMs: 30, test: () => null });
  assert.equal(result.kind, "timeout");
  assert.equal(result.lastSeq, 0);
});

test("waitFor 在串口断开时立即返回 disconnected", async () => {
  const waiting = service.waitFor({ timeoutMs: 5000, test: () => null });
  testBinding.unplug(PATH);
  const result = await waiting;
  assert.equal(result.kind, "disconnected");
});

test("waitFor 在串口未打开时立即返回 disconnected", async () => {
  await service.close();
  const result = await service.waitFor({ timeoutMs: 5000, test: () => null });
  assert.equal(result.kind, "disconnected");
});

test("waitFor 被取消时返回 aborted 并释放订阅", async () => {
  const controller = new AbortController();
  const waiting = service.waitFor({ timeoutMs: 5000, signal: controller.signal, test: () => null });
  controller.abort();
  const result = await waiting;
  assert.equal(result.kind, "aborted");
});
