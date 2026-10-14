import assert from "node:assert/strict";
import { describe, test } from "vitest";
import { CaptureFormatError, readCaptureData } from "../src/pcap/pcapng.js";
import { buildClassicPcap, buildUsbmonPcapng, type FixtureEvent } from "./fixtures.js";

const EVENTS: FixtureEvent[] = [
  { ts: 1_000, endpoint: 0x81, type: 3, complete: false, declaredLength: 4 },
  { ts: 2_000, endpoint: 0x81, type: 3, complete: true, data: new Uint8Array([1, 2, 3, 4]) },
];

function collect(data: Uint8Array) {
  const records: Array<{ linkType: number; tsMicros: number; length: number }> = [];
  const result = readCaptureData(data, (record) => {
    records.push({ linkType: record.linkType, tsMicros: record.tsMicros, length: record.data.length });
  });
  return { records, result };
}

describe("pcapng 容器", () => {
  test("读出接口的描述符块与增强包块，时间戳按微秒还原", () => {
    const { records, result } = collect(buildUsbmonPcapng(EVENTS));
    assert.equal(result.format, "pcapng");
    assert.equal(result.interfaces.length, 1);
    assert.equal(result.interfaces[0]?.linkType, 220);
    assert.equal(result.records, 2);
    assert.equal(result.truncated, false);
    assert.deepEqual(records, [
      { linkType: 220, tsMicros: 1_000, length: 64 },
      { linkType: 220, tsMicros: 2_000, length: 68 },
    ]);
  });

  test("文件截断时保留已解析的记录并置 truncated", () => {
    const full = buildUsbmonPcapng(EVENTS);
    const { records, result } = collect(full.subarray(0, full.length - 10));
    assert.equal(result.truncated, true);
    assert.equal(records.length, 1);
    assert.ok(result.warnings.some((warning) => warning.includes("截断")));
  });

  test("块首尾长度不一致时按截断处理，不静默相信头部", () => {
    const data = buildUsbmonPcapng(EVENTS);
    const corrupted = Uint8Array.from(data);
    const view = new DataView(corrupted.buffer, corrupted.byteOffset, corrupted.byteLength);
    // 段头 28 + 接口块 20 = 48 起是第一个增强包块；尾部长度字段在块末尾 4 字节。
    const firstPacketBlock = 48;
    const total = view.getUint32(firstPacketBlock + 4, true);
    corrupted[firstPacketBlock + total - 1] = 0xff;
    const { result } = collect(corrupted);
    assert.equal(result.truncated, true);
  });

  test("非抓包文件报 unsupportedFormat（不猜格式）", () => {
    assert.throws(
      () => collect(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])),
      (error: unknown) => error instanceof CaptureFormatError && error.code === "unsupportedFormat",
    );
  });

  test("经典 pcap 容器（小端 magic）走同一条记录接口", () => {
    const { records, result } = collect(buildClassicPcap(EVENTS));
    assert.equal(result.format, "pcap");
    assert.equal(result.records, 2);
    assert.equal(records[1]?.tsMicros, 2_000);
    assert.equal(records[1]?.linkType, 220);
  });
});
