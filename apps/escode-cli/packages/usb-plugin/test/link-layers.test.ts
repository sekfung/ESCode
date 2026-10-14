import assert from "node:assert/strict";
import { describe, test } from "vitest";
import { readCaptureData } from "../src/pcap/pcapng.js";
import { toExchanges } from "../src/pcap/reassemble.js";
import { decodeUsbpcapRecord } from "../src/pcap/usbpcap.js";
import { decodeUsbmonRecord, type LinkProblem } from "../src/pcap/usbmon.js";
import type { UsbTransfer } from "../src/types.js";
import {
  buildUsbpcapPcapng,
  buildUsbmonPcapng,
  controlExchange,
  deviceDescriptor,
  type FixtureEvent,
} from "./fixtures.js";

function usbmonTransfers(data: Uint8Array, linkType = 220): { transfers: UsbTransfer[]; problems: LinkProblem[] } {
  const transfers: UsbTransfer[] = [];
  const problems: LinkProblem[] = [];
  readCaptureData(data, (record) => {
    const transfer = decodeUsbmonRecord(linkType, record.data, record.tsMicros, problems);
    if (transfer) transfers.push(transfer);
  });
  return { transfers, problems };
}

describe("usbmon 链路层", () => {
  test("控制传输：提交侧带 SETUP，完成侧带数据，配对成一条逻辑报文", () => {
    const events = controlExchange({ ts: 1_000, wValue: 0x0100, data: deviceDescriptor(0x072f, 0x223b) });
    const { transfers, problems } = usbmonTransfers(buildUsbmonPcapng(events));
    assert.equal(problems.length, 0);
    assert.equal(transfers.length, 2);
    assert.deepEqual(transfers[0]?.setup, {
      bmRequestType: 0x80,
      bRequest: 0x06,
      wValue: 0x0100,
      wIndex: 0,
      wLength: 18,
    });
    assert.equal(transfers[0]?.direction, "in");
    assert.equal(transfers[0]?.type, "control");
    assert.equal(transfers[1]?.data.length, 18);

    const { exchanges, warnings } = toExchanges(transfers);
    assert.equal(warnings.length, 0);
    assert.equal(exchanges.length, 1);
    assert.equal(exchanges[0]?.setup?.bRequest, 0x06);
    assert.equal(exchanges[0]?.data.length, 18);
    assert.equal(exchanges[0]?.busDev, "1-5");
    assert.equal(exchanges[0]?.durationUs, 200);
  });

  test("批量 OUT 的数据在提交事件里也要取到（usbmon 不把 OUT 数据放在完成侧）", () => {
    const payload = new Uint8Array([0x6f, 0x02, 0x00, 0x00, 0x00]);
    const events: FixtureEvent[] = [
      { ts: 1_000, endpoint: 0x03, type: 3, complete: false, data: payload, urbId: 42 },
      { ts: 1_400, endpoint: 0x03, type: 3, complete: true, urbId: 42, declaredLength: payload.length },
    ];
    const { exchanges } = toExchanges(usbmonTransfers(buildUsbmonPcapng(events)).transfers);
    assert.equal(exchanges.length, 1);
    assert.equal(exchanges[0]?.direction, "out");
    assert.deepEqual([...(exchanges[0]?.data ?? [])], [...payload]);
    assert.equal(exchanges[0]?.status, 0);
  });

  test("未知传输类型与填充包不进入分析，填充包也不报问题", () => {
    const data = buildUsbmonPcapng([
      { ts: 1_000, endpoint: 0x81, type: 3, complete: true, data: new Uint8Array([1]) },
      { ts: 2_000, endpoint: 0x81, type: 3, complete: true, data: new Uint8Array([2]) },
    ]);
    assert.deepEqual(usbmonTransfers(data).problems, []);
    assert.equal(usbmonTransfers(data).transfers.length, 2);

    // 段头 28 + 接口块 20 + 增强包块头 8 + 固定字段 20 = 76 起是第一个 usbmon 头。
    const firstHeader = 76;
    const filler = Uint8Array.from(data);
    filler[firstHeader + 8] = 0x40; // '@' 填充包：静默跳过
    assert.equal(usbmonTransfers(filler).transfers.length, 1);
    assert.deepEqual(usbmonTransfers(filler).problems, []);

    const unknownType = Uint8Array.from(data);
    unknownType[firstHeader + 9] = 0x09; // 未知传输类型：报问题，不当有效报文
    const result = usbmonTransfers(unknownType);
    assert.equal(result.transfers.length, 1);
    assert.ok(result.problems.some((problem) => problem.message.includes("传输类型")));
  });

  test("短传输：提交声明长度与完成实际长度不一致时两边都留痕", () => {
    const events: FixtureEvent[] = [
      { ts: 1_000, endpoint: 0x81, type: 3, complete: false, declaredLength: 64, urbId: 7 },
      {
        ts: 1_100,
        endpoint: 0x81,
        type: 3,
        complete: true,
        data: new Uint8Array([1, 2, 3]),
        declaredLength: 3,
        urbId: 7,
      },
    ];
    const { exchanges } = toExchanges(usbmonTransfers(buildUsbmonPcapng(events)).transfers);
    assert.equal(exchanges[0]?.expectedLength, 64);
    assert.equal(exchanges[0]?.actualLength, 3);
  });
});

describe("USBPcap 链路层", () => {
  test("控制传输两个包：SETUP 包的载荷前 8 字节是 SETUP，COMPLETE 包带 DATA IN", () => {
    const setup = new Uint8Array([0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00]);
    const descriptor = deviceDescriptor(0x046d, 0xc52b);
    const transfers: UsbTransfer[] = [];
    const problems: LinkProblem[] = [];
    readCaptureData(
      buildUsbpcapPcapng([
        { ts: 1_000, endpoint: 0x80, transfer: 2, stage: 0, complete: false, data: setup, irpId: 99 },
        { ts: 1_300, endpoint: 0x80, transfer: 2, stage: 3, complete: true, data: descriptor, irpId: 99 },
      ]),
      (record) => {
        const transfer = decodeUsbpcapRecord(record.data, record.tsMicros, problems);
        if (transfer) transfers.push(transfer);
      },
    );
    assert.equal(problems.length, 0);
    assert.equal(transfers.length, 2);
    assert.deepEqual(transfers[0]?.setup, {
      bmRequestType: 0x80,
      bRequest: 0x06,
      wValue: 0x0100,
      wIndex: 0,
      wLength: 0x12,
    });
    // SETUP 阶段包本身不再把 setup 8 字节当成数据正文。
    assert.equal(transfers[0]?.data.length, 0);
    assert.equal(transfers[1]?.complete, true);
    assert.equal(transfers[1]?.data.length, 18);

    const { exchanges } = toExchanges(transfers);
    assert.equal(exchanges.length, 1);
    assert.equal(exchanges[0]?.data.length, 18);
    assert.equal(exchanges[0]?.setup?.wValue, 0x0100);
  });

  test("info 位 0 的含义：0 是提交、1 是完成（不能按字段名直觉反着读）", () => {
    const transfers: UsbTransfer[] = [];
    readCaptureData(
      buildUsbpcapPcapng([
        { ts: 1_000, endpoint: 0x81, transfer: 1, complete: false, data: new Uint8Array([1, 2]) },
        { ts: 2_000, endpoint: 0x81, transfer: 1, complete: true, data: new Uint8Array([3, 4]) },
      ]),
      (record) => {
        const transfer = decodeUsbpcapRecord(record.data, record.tsMicros, []);
        if (transfer) transfers.push(transfer);
      },
    );
    assert.deepEqual(
      transfers.map((transfer) => transfer.complete),
      [false, true],
    );
    assert.equal(transfers[0]?.type, "interrupt");
  });

  test("IRP_INFO / UNKNOWN 不是 USB 事务，跳过且不计问题", () => {
    const problems: LinkProblem[] = [];
    const transfers: UsbTransfer[] = [];
    readCaptureData(
      buildUsbpcapPcapng([
        { ts: 1_000, endpoint: 0x81, transfer: 3, complete: false, data: new Uint8Array([1]) },
      ]),
      (record) => {
        // 把 transfer 字段改成 0xFE（IRP_INFO）：应当静默跳过。
        const patched = Uint8Array.from(record.data);
        patched[22] = 0xfe;
        const transfer = decodeUsbpcapRecord(patched, record.tsMicros, problems);
        if (transfer) transfers.push(transfer);
      },
    );
    assert.deepEqual(transfers, []);
    assert.deepEqual(problems, []);
  });
});
