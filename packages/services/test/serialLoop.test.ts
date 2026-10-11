import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { MockBinding } from "@serialport/binding-mock";
import { SerialError } from "../src/serial/serial.js";
import { createSerialService, type SerialService } from "../src/serial/serialService.js";
import { createTestBinding, TEST_SERIAL_CONFIG, waitFor } from "./serialTestBinding.js";

const PATH = "/dev/ttyLOOP0";
let service: SerialService | undefined;
let testBinding: ReturnType<typeof createTestBinding>;

async function setup(options: { writeDelayMs?: number } = {}) {
  MockBinding.reset();
  MockBinding.createPort(PATH);
  testBinding = createTestBinding(options);
  service = createSerialService({
    loadBinding: async () => testBinding.binding,
    pollIntervalMs: 5,
    coalesceWindowMs: 0,
  });
  await service.open({ path: PATH, config: TEST_SERIAL_CONFIG });
  return service;
}

afterEach(async () => {
  await service?.disposeAllAndWait();
  service = undefined;
});

const txCount = async (svc: SerialService) =>
  (await svc.getSnapshot({ path: PATH })).chunks.filter((chunk) => chunk.direction === "tx").length;

test("按间隔发送指定次数后自动停止，进度进入状态", async () => {
  const svc = await setup();
  await svc.startLoop({ path: PATH, bytes: Buffer.from("P"), intervalMs: 10, count: 5 });
  assert.deepEqual((await svc.getSnapshot({ path: PATH })).status.loop?.count, 5);
  await waitFor(
    async () => (await svc.getSnapshot({ path: PATH })).status.loop === undefined,
    3000,
  );
  assert.equal(await txCount(svc), 5);
});

test("无次数限制时持续发送，stopLoop 停止", async () => {
  const svc = await setup();
  await svc.startLoop({ path: PATH, bytes: Buffer.from("P"), intervalMs: 10 });
  await waitFor(async () => (await txCount(svc)) >= 3);
  await svc.stopLoop({ path: PATH });
  const stopped = await txCount(svc);
  await new Promise((resolve) => setTimeout(resolve, 60));
  assert.equal(await txCount(svc), stopped);
  assert.equal((await svc.getSnapshot({ path: PATH })).status.loop, undefined);
});

test("关闭串口时循环自动停止", async () => {
  const svc = await setup();
  await svc.startLoop({ path: PATH, bytes: Buffer.from("P"), intervalMs: 10 });
  await svc.close({ path: PATH });
  assert.equal((await svc.getSnapshot({ path: PATH })).status.loop, undefined);
  const count = await txCount(svc);
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(await txCount(svc), count);
});

test("上一笔写入未完成时跳过本次，不积压", async () => {
  const svc = await setup({ writeDelayMs: 45 });
  await svc.startLoop({ path: PATH, bytes: Buffer.from("P"), intervalMs: 10 });
  await new Promise((resolve) => setTimeout(resolve, 230));
  await svc.stopLoop({ path: PATH });
  const sent = await txCount(svc);
  // 230ms / 45ms 写入耗时 ≈ 5 次；若排队积压会接近 230/10 = 23 次
  assert.ok(sent >= 3 && sent <= 7, `sent=${sent}`);
});

test("再次启动替换旧任务；参数非法或串口未打开时拒绝", async () => {
  const svc = await setup();
  await svc.startLoop({ path: PATH, bytes: Buffer.from("A"), intervalMs: 1000 });
  await svc.startLoop({ path: PATH, bytes: Buffer.from("B"), intervalMs: 10, count: 2 });
  await waitFor(
    async () => (await svc.getSnapshot({ path: PATH })).status.loop === undefined,
    3000,
  );
  const sent = (await svc.getSnapshot({ path: PATH })).chunks.map((chunk) =>
    Buffer.from(chunk.bytes).toString(),
  );
  // 启动即发送一次：A 在被替换前已发出，之后只发 B
  assert.deepEqual(sent, ["A", "B", "B"]);
  const rejects = (params: Parameters<SerialService["startLoop"]>[0], code: string) =>
    assert.rejects(
      svc.startLoop(params),
      (error: unknown) => error instanceof SerialError && error.code === code,
    );
  await rejects({ path: PATH, bytes: Buffer.from("x"), intervalMs: 5 }, "invalidInput");
  await rejects({ path: PATH, bytes: Buffer.from(""), intervalMs: 10 }, "invalidInput");
  await rejects({ path: PATH, bytes: Buffer.from("x"), intervalMs: 10, count: 0 }, "invalidInput");
  await svc.close({ path: PATH });
  await rejects({ path: PATH, bytes: Buffer.from("x"), intervalMs: 10 }, "notOpen");
});

test("进度事件节流：不超过每 250ms 一次（启停立即推送）", async () => {
  const svc = await setup();
  const loopEvents: number[] = [];
  svc.onStatus((status) => {
    if (status.loop) loopEvents.push(Date.now());
  });
  await svc.startLoop({ path: PATH, bytes: Buffer.from("P"), intervalMs: 10 });
  await new Promise((resolve) => setTimeout(resolve, 400));
  await svc.stopLoop({ path: PATH });
  // 400ms 内：启动 1 次 + 进度最多 2 次
  assert.ok(loopEvents.length <= 3, `events=${loopEvents.length}`);
});
