import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { MockBinding } from "@serialport/binding-mock";
import { SerialError } from "../src/serial/serial.js";
import { createSerialService, type SerialService } from "../src/serial/serialService.js";
import { createTestBinding, TEST_SERIAL_CONFIG, waitFor } from "./serialTestBinding.js";

const PATH = "/dev/ttySIG0";
let testBinding: ReturnType<typeof createTestBinding>;
let service: SerialService;

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
  testBinding.setCalls.length = 0;
});

afterEach(async () => {
  await service.disposeAllAndWait();
});

const signals = async () => (await service.getSnapshot({ path: PATH })).status.signals;
const setOptions = () =>
  testBinding.setCalls.map((call) => ({ dtr: call.options.dtr, rts: call.options.rts }));

test("打开后信号初值为 DTR=1、RTS=1，并在状态中返回", async () => {
  assert.deepEqual(await signals(), { dtr: true, rts: true });
});

test("setSignals 经 binding set 生效并广播到状态事件", async () => {
  const statuses: unknown[] = [];
  service.onStatus((status) => statuses.push(status.signals));
  const result = await service.setSignals({ path: PATH, dtr: false });
  assert.deepEqual(result, { dtr: false, rts: true });
  assert.deepEqual(setOptions(), [{ dtr: false, rts: true }]);
  assert.deepEqual(await signals(), { dtr: false, rts: true });
  assert.deepEqual(statuses.at(-1), { dtr: false, rts: true });
});

test("ESP32 复位脉冲按时序切换信号，结束后恢复脉冲前状态", async () => {
  await service.setSignals({ path: PATH, dtr: false, rts: false });
  testBinding.setCalls.length = 0;
  const result = await service.setSignals({ path: PATH, pulse: "esp32" });
  assert.deepEqual(result, { dtr: false, rts: false });
  assert.deepEqual(setOptions(), [
    { dtr: false, rts: true },
    { dtr: true, rts: false },
    { dtr: false, rts: false },
    { dtr: false, rts: false },
  ]);
  const [, second, third] = testBinding.setCalls;
  const gap = (a?: { at: number }, b?: { at: number }) => (b?.at ?? 0) - (a?.at ?? 0);
  assert.ok(gap(testBinding.setCalls[0], second) >= 95, "DTR=0,RTS=1 至少保持 100ms");
  assert.ok(gap(second, third) >= 45, "DTR=1,RTS=0 至少保持 50ms");
});

test("Arduino 复位脉冲：DTR=1 保持 100ms 后 DTR=0，再恢复", async () => {
  await service.setSignals({ path: PATH, pulse: "arduino" });
  assert.deepEqual(setOptions(), [
    { dtr: true, rts: true },
    { dtr: false, rts: true },
    { dtr: true, rts: true },
  ]);
  assert.ok(testBinding.setCalls[1]!.at - testBinding.setCalls[0]!.at >= 95);
  assert.deepEqual(await signals(), { dtr: true, rts: true });
});

test("脉冲进行中的 setSignals 排到脉冲结束后执行", async () => {
  const pulse = service.setSignals({ path: PATH, pulse: "arduino" });
  const change = service.setSignals({ path: PATH, rts: false });
  await Promise.all([pulse, change]);
  assert.deepEqual(setOptions().at(-1), { dtr: true, rts: false });
  assert.deepEqual(await signals(), { dtr: true, rts: false });
});

test("开启 RTS/CTS 流控时拒绝手动设置 RTS；未打开时返回 notOpen", async () => {
  await service.close({ path: PATH });
  await assert.rejects(
    service.setSignals({ path: PATH, dtr: true }),
    (error: unknown) => error instanceof SerialError && error.code === "notOpen",
  );
  await service.open({ path: PATH, config: { ...TEST_SERIAL_CONFIG, rtscts: true } });
  await assert.rejects(
    service.setSignals({ path: PATH, rts: false }),
    (error: unknown) => error instanceof SerialError && error.code === "invalidInput",
  );
  await service.setSignals({ path: PATH, dtr: false });
});

test("自动重连后恢复断开前的信号状态", async () => {
  await service.setSignals({ path: PATH, dtr: false, rts: false });
  testBinding.unplug(PATH);
  await waitFor(
    async () => (await service.getSnapshot({ path: PATH })).status.state === "disconnected",
  );
  testBinding.setCalls.length = 0;
  testBinding.replug(PATH);
  await waitFor(async () => (await service.getSnapshot({ path: PATH })).status.state === "open");
  assert.deepEqual(await signals(), { dtr: false, rts: false });
  assert.deepEqual(setOptions(), [{ dtr: false, rts: false }]);
});
