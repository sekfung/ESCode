import assert from "node:assert/strict";
import { describe, test } from "vitest";
import { analyzeCapture } from "../src/analysis.js";
import { CaptureFormatError } from "../src/pcap/pcapng.js";
import {
  buildUsbmonPcapng,
  bulkExchange,
  ccidDataBlock,
  ccidXfrBlock,
  configWithInterface,
  controlExchange,
  deviceDescriptor,
  type FixtureEvent,
} from "./fixtures.js";

/** 一张「CCID 读卡器」抓包：描述符 + 一次成功交换 + 一次 SW=6A82 的失败交换。 */
function ccidCapture(): Uint8Array {
  const events: FixtureEvent[] = [
    ...controlExchange({ ts: 1_000, wValue: 0x0100, data: deviceDescriptor(0x072f, 0x223b) }),
    ...controlExchange({ ts: 2_000, wValue: 0x0200, data: configWithInterface(0x0b, 0x00, 0x00) }),
    ...bulkExchange({ ts: 10_000, endpoint: 0x03, data: ccidXfrBlock(7, [0x00, 0xa4, 0x04, 0x00, 0x00]) }),
    ...bulkExchange({ ts: 10_400, endpoint: 0x83, data: ccidDataBlock(7, [0x90, 0x00]) }),
    ...bulkExchange({ ts: 20_000, endpoint: 0x03, data: ccidXfrBlock(8, [0x00, 0xb0, 0x00, 0x00, 0x04]) }),
    ...bulkExchange({ ts: 20_400, endpoint: 0x83, data: ccidDataBlock(8, [0x6a, 0x82]) }),
  ];
  return buildUsbmonPcapng(events);
}

describe("解码视图", () => {
  test("summary：识别设备类别与协议，列出异常并给出可执行的下一步", () => {
    const output = analyzeCapture(ccidCapture(), { protocol: "auto", view: "summary" });
    assert.match(output.text, /1-5/);
    assert.match(output.text, /072f:223b/);
    assert.match(output.text, /smart-card-ccid/);
    assert.match(output.text, /异常：1 处/);
    assert.match(output.text, /6A82/);
    assert.equal(output.meta.protocol, "auto");
    assert.ok(Number(output.meta.messages) >= 2);
    assert.equal(output.meta.cursor, null);
  });

  test("timeline：分页给出编号，并要求用 cursor 取下一页", () => {
    const first = analyzeCapture(ccidCapture(), { protocol: "ccid", view: "timeline", limit: 1 });
    assert.match(first.text, /^报文 1–1 \/ 共 2 条/mu);
    assert.match(first.text, /#1 .*XfrBlock/);
    assert.match(first.text, /下一页：cursor=2/);
    assert.equal(first.meta.cursor, 2);
    assert.equal(first.meta.truncated, true);

    const second = analyzeCapture(ccidCapture(), {
      protocol: "ccid",
      view: "timeline",
      limit: 1,
      cursor: "2",
    });
    assert.match(second.text, /^报文 2–2 \/ 共 2 条/mu);
    assert.equal(second.meta.cursor, null);
    assert.equal(second.meta.truncated, false);
  });

  test("timeline：失败的状态字原样出现在报文行里（不隐藏关键证据）", () => {
    const output = analyzeCapture(ccidCapture(), { protocol: "ccid", view: "timeline" });
    assert.match(output.text, /6A82/);
    assert.match(output.text, /⚠/);
  });

  test("message：按编号取单条正文，正文长度受 payloadBytes 限制", () => {
    const output = analyzeCapture(ccidCapture(), {
      protocol: "ccid",
      view: "message",
      index: 1,
      payloadBytes: 8,
    });
    assert.equal(output.meta.found, true);
    assert.match(output.text, /^#1 /mu);
    assert.match(output.text, /正文（5 字节/);
    assert.match(output.text, /归属报文：#/);

    const missing = analyzeCapture(ccidCapture(), { protocol: "ccid", view: "message", index: 99 });
    assert.equal(missing.meta.found, false);
    assert.match(missing.text, /没有编号为 #99 的报文/);
  });

  test("filter：onlyErrors 只留含异常的报文，busDev 与 direction 也能收窄", () => {
    const errors = analyzeCapture(ccidCapture(), {
      protocol: "ccid",
      view: "timeline",
      filter: { onlyErrors: true },
    });
    assert.match(errors.text, /共 1 条/);
    assert.match(errors.text, /6A82/);

    const otherDevice = analyzeCapture(ccidCapture(), {
      protocol: "ccid",
      view: "timeline",
      filter: { busDev: "2-9" },
    });
    assert.match(otherDevice.text, /共 0 条/);
  });

  test("protocol=raw 在没有协议知识时仍能给出端点、长度与 SETUP", () => {
    const output = analyzeCapture(ccidCapture(), { protocol: "raw", view: "timeline", limit: 500 });
    assert.match(output.text, /Control/);
    assert.match(output.text, /Bulk/);
    assert.match(output.text, /GET_DESCRIPTOR/);
  });

  test("抓包里没有描述符时明确提示需要重新插拔，而不是猜设备类别", () => {
    const capture = buildUsbmonPcapng([
      ...bulkExchange({ ts: 1_000, endpoint: 0x03, data: ccidXfrBlock(1, [0x00, 0xa4, 0x04, 0x00, 0x00]) }),
      ...bulkExchange({ ts: 1_400, endpoint: 0x83, data: ccidDataBlock(1, [0x90, 0x00]) }),
    ]);
    const output = analyzeCapture(capture, { protocol: "auto", view: "summary" });
    assert.match(output.text, /没有描述符/);
    assert.match(output.text, /重新插拔/);
  });

  test("非 USB 链路类型报 unsupportedFormat，并说明支持哪些链路类型", () => {
    assert.throws(
      () => analyzeCapture(buildUsbmonPcapng([], 1), { protocol: "auto", view: "summary" }),
      (error: unknown) =>
        error instanceof CaptureFormatError &&
        error.code === "unsupportedFormat" &&
        /189\/220|249/u.test(error.message),
    );
  });
  test("时间按相对首条报文的偏移渲染：墙钟时间戳的抓包也不会显示成 +1700000000s", () => {
    // USBPcap 与部分 usbmon 抓包的时间戳是墙钟（约 1.7e15 µs）。绝对值对协议分析没有意义，
    // 视图必须渲染相对偏移，否则「谁在前谁在后」会被一长串数字淹没。
    const capture = buildUsbmonPcapng([
      ...bulkExchange({ ts: 1_700_000_000_100_000, endpoint: 0x83, data: ccidDataBlock(1, [0x90, 0x00]) }),
      ...bulkExchange({ ts: 1_700_000_000_300_000, endpoint: 0x83, data: ccidDataBlock(2, [0x90, 0x00]) }),
    ]);
    const timeline = analyzeCapture(capture, { protocol: "ccid", view: "timeline" });
    assert.match(timeline.text, /#1 \+0\.000s/u);
    assert.match(timeline.text, /#2 \+0\.200s/u);
    assert.doesNotMatch(timeline.text, /\+1700000000\./u);
    // 时间范围那行属于 summary 视图，timeline 只有逐条偏移。
    const summary = analyzeCapture(capture, { protocol: "ccid", view: "summary" });
    assert.match(summary.text, /时间范围：\+0\.000s – \+0\.200s/u);
  });

  test("异常只在报文行里出现一次（不在 summary 与视图层各写一遍）", () => {
    const capture = buildUsbmonPcapng([
      { ts: 1_000, endpoint: 0x81, type: 3, complete: true, data: new Uint8Array([1]), status: -32 },
    ]);
    const timeline = analyzeCapture(capture, { protocol: "raw", view: "timeline" });
    const occurrences = timeline.text.split("状态码 -32").length - 1;
    assert.equal(occurrences, 1, timeline.text);
  });
});
