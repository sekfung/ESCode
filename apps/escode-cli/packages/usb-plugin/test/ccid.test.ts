import { describe, expect, test } from "vitest";
import { decodeCcidMessages } from "../src/decode/ccid.js";
import { protocolsForInterfaceClass } from "../src/types.js";
import type { DecodeContext, DecodedMessage, UsbExchange } from "../src/types.js";

/** 按 CCID 规范手算的字节向量；这里只构造抓包真会出现的字段。 */
function exchange(
  index: number,
  direction: "in" | "out",
  data: number[],
  type: UsbExchange["type"] = "bulk",
): UsbExchange {
  return {
    index,
    ts: index * 1000,
    bus: 1,
    device: 4,
    busDev: "1-4",
    endpoint: direction === "in" ? 0x81 : 0x02,
    direction,
    type,
    data: Uint8Array.from(data),
    status: 0,
  };
}

function context(exchanges: UsbExchange[], classCode = 0x0b): DecodeContext {
  return {
    exchanges,
    device: {
      busDev: "1-4",
      bus: 1,
      device: 4,
      interfaces: [
        {
          number: 0,
          classCode,
          subClassCode: 0,
          protocolCode: 0,
          protocols: protocolsForInterfaceClass(classCode),
        },
      ],
      descriptorSource: "capture",
    },
  };
}

function only(messages: DecodedMessage[]): DecodedMessage {
  expect(messages).toHaveLength(1);
  const message = messages[0];
  if (message === undefined) throw new Error("期望恰好一条消息");
  return message;
}

function hasAnomaly(message: DecodedMessage, fragment: string): boolean {
  return message.anomalies.some((item) => item.includes(fragment));
}

// PC_to_RDR_XfrBlock: 6F | dwLength=00000007 | bSlot=00 | bSeq=01 | bBWI=00 | wLevelParameter=0000
// 载荷 = SELECT MF 的 case 3 APDU：00 A4 00 00 02 3F 00
const XFR_SELECT = [
  0x6f, 0x07, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xa4, 0x00, 0x00, 0x02, 0x3f, 0x00,
];
// RDR_to_PC_DataBlock: 80 | dwLength=00000002 | bSlot=00 | bSeq=01 | bStatus=00 | bError=00 | bChainParameter=00
const DATA_OK = [0x80, 0x02, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0x00, 0x90, 0x00];
const DATA_NOT_FOUND = [0x80, 0x02, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0x00, 0x6a, 0x82];

describe("decodeCcidMessages", () => {
  test("XfrBlock + 成功 DataBlock 合并为一条逻辑 APDU 交换", () => {
    const message = only(
      decodeCcidMessages(context([exchange(1, "out", XFR_SELECT), exchange(2, "in", DATA_OK)])),
    );
    expect(message.kind).toBe("PC_to_RDR_XfrBlock");
    expect(message.exchanges).toEqual([1, 2]);
    expect(message.direction).toBe("out");
    expect(message.summary).toContain("SELECT");
    expect(message.summary).toContain("0x9000");
    expect(message.summary).toContain("成功");
    expect(message.detail.join("\n")).toContain("wLevelParameter=0x0000");
    expect(message.detail.join("\n")).toContain("Lc=2, data=3F 00（case 3）");
    // bStatus 的 bmCommandStatus / bmICCStatus 两个子域都要解出来
    expect(message.detail.join("\n")).toContain("bStatus=0x00（命令处理成功, ICC 在位且激活）");
    expect(message.anomalies).toEqual([]);
    // 正文是命令侧的原始 APDU 字节
    expect([...(message.payload ?? [])]).toEqual([0x00, 0xa4, 0x00, 0x00, 0x02, 0x3f, 0x00]);
  });

  test("失败 SW1SW2 计入 anomalies，并原样出现在 summary", () => {
    const message = only(
      decodeCcidMessages(
        context([exchange(1, "out", XFR_SELECT), exchange(2, "in", DATA_NOT_FOUND)]),
      ),
    );
    expect(message.summary).toContain("0x6A82");
    expect(message.summary).toContain("文件未找到");
    expect(hasAnomaly(message, "SW1SW2=0x6A82")).toBe(true);
    expect(hasAnomaly(message, "文件未找到")).toBe(true);
    expect(message.detail.join("\n")).toContain("SW1SW2=0x6A82");
  });

  test("链式块合并成一条交换，detail 记录块数与原始链位", () => {
    // 请求链：第一块 wLevelParameter=0x0010（还有后续块），第二块 0x0000
    const reqA = [0x6f, 0x04, 0, 0, 0, 0x00, 0x02, 0x00, 0x10, 0x00, 0x00, 0xa4, 0x00, 0x00];
    const reqB = [0x6f, 0x03, 0, 0, 0, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x3f, 0x00];
    // 响应链：第一块 bChainParameter=0x01，第二块 0x00，SW 只在最后一块
    const respA = [0x80, 0x02, 0, 0, 0, 0x00, 0x02, 0x00, 0x00, 0x01, 0xde, 0xad];
    const respB = [0x80, 0x04, 0, 0, 0, 0x00, 0x02, 0x00, 0x00, 0x00, 0xbe, 0xef, 0x90, 0x00];
    const message = only(
      decodeCcidMessages(
        context([
          exchange(1, "out", reqA),
          exchange(2, "out", reqB),
          exchange(3, "in", respA),
          exchange(4, "in", respB),
        ]),
      ),
    );
    const text = message.detail.join("\n");
    expect(message.exchanges).toEqual([1, 2, 3, 4]);
    expect(message.summary).toContain("SELECT");
    expect(message.summary).toContain("成功");
    expect(text).toContain("wLevelParameter=0x0010");
    expect(text).toContain("bChainParameter=0x01");
    expect(text).toContain("链式合并: 请求 2 块 + 响应 2 块");
    expect(text).toContain("响应: 数据 4 字节, SW1SW2=0x9000");
    expect([...(message.payload ?? [])]).toEqual([0x00, 0xa4, 0x00, 0x00, 0x02, 0x3f, 0x00]);
    expect(message.anomalies).toEqual([]);
  });

  test("链没走完就结束计入 anomalies", () => {
    // 单个 XfrBlock 声明还有后续块，抓包到这里结束
    const orphan = [0x6f, 0x04, 0, 0, 0, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0xa4, 0x00, 0x00];
    const message = only(decodeCcidMessages(context([exchange(1, "out", orphan)])));
    expect(hasAnomaly(message, "命令链未在抓包内结束")).toBe(true);
  });

  test("未知消息类型给 unknown(0xNN) 与 anomaly，不抛异常", () => {
    // 0x64 在 CCID Rev 1.1 里没有定义
    const raw = [0x64, 0x01, 0, 0, 0, 0x00, 0x07, 0x01, 0x02, 0x03, 0xaa];
    const message = only(decodeCcidMessages(context([exchange(1, "out", raw)])));
    expect(message.kind).toBe("unknown(0x64)");
    expect(hasAnomaly(message, "未收录的消息类型 bMessageType=0x64")).toBe(true);
    expect([...(message.payload ?? [])]).toEqual(raw);
  });

  test("dwLength 与实际载荷不一致 / 头部被截断都给 anomaly，不抛异常", () => {
    const declaredTooLong = [
      0x6f, 0x09, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xa4, 0x00, 0x00, 0x02, 0x3f, 0x00,
    ];
    const mismatch = only(
      decodeCcidMessages(
        context([exchange(1, "out", declaredTooLong), exchange(2, "in", DATA_OK)]),
      ),
    );
    expect(hasAnomaly(mismatch, "dwLength=9 与载荷 7 字节不符")).toBe(true);
    // 长度字段不可信时仍按实际字节解出 APDU
    expect(mismatch.summary).toContain("SELECT");

    const truncated = only(decodeCcidMessages(context([exchange(1, "out", [0x6f, 0x05, 0x00])])));
    expect(truncated.kind).toBe("PC_to_RDR_XfrBlock");
    expect(hasAnomaly(truncated, "消息头不足 10 字节（实际 3 字节）")).toBe(true);
  });

  test("IccPowerOn 响应解出 ATR", () => {
    // PC_to_RDR_IccPowerOn：dwLength=0，bPowerSelect=00（自动）
    const powerOn = [0x62, 0x00, 0, 0, 0, 0x00, 0x05, 0x00, 0x00, 0x00];
    // ATR：TS=3B、T0=95（TA1+TD1，K=5）、TA1=96、TD1=80（TA2，T=0）、TA2=00、5 个历史字节
    const atr = [
      0x80, 0x0a, 0, 0, 0, 0x00, 0x05, 0x00, 0x00, 0x00, 0x3b, 0x95, 0x96, 0x80, 0x00, 0x00, 0x01,
      0x02, 0x03, 0x04,
    ];
    const message = only(
      decodeCcidMessages(context([exchange(1, "out", powerOn), exchange(2, "in", atr)])),
    );
    const text = message.detail.join("\n");
    expect(message.kind).toBe("PC_to_RDR_IccPowerOn");
    expect(message.summary).toContain("ATR");
    expect(message.summary).toContain("直接约定");
    expect(text).toContain("TA1=0x96");
    expect(text).toContain("TD1=0x80");
    expect(text).toContain("TA2=0x00");
    expect(text).toContain("历史字节=00 01 02 03 04");
    expect(message.anomalies).toEqual([]);
  });

  test("多词 INS 名称不被截断（READ BINARY）", () => {
    // 00 B0 00 00 04 01 02 03 04 = READ BINARY，Lc=4
    const readBinary = [
      0x6f, 0x09, 0, 0, 0, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0xb0, 0x00, 0x00, 0x04, 0x01, 0x02,
      0x03, 0x04,
    ];
    const message = only(
      decodeCcidMessages(context([exchange(1, "out", readBinary), exchange(2, "in", DATA_OK)])),
    );
    expect(message.summary).toContain("READ BINARY");
    expect(message.detail.join("\n")).toContain("INS=0xB0 READ BINARY");
  });

  test("2 字节的中断端 NotifySlotChange 不按头部截断报错", () => {
    const message = only(
      decodeCcidMessages(context([exchange(1, "in", [0x50, 0x03], "interrupt")])),
    );
    expect(message.kind).toBe("RDR_to_PC_NotifySlotChange");
    expect(message.direction).toBe("in");
    expect(message.detail.join("\n")).toContain("中断端消息（非 10 字节通用头）");
    expect(message.anomalies).toEqual([]);
  });

  test("非 CCID 接口的设备整体跳过", () => {
    const hid = context([exchange(1, "out", XFR_SELECT)], 0x03);
    expect(decodeCcidMessages(hid)).toEqual([]);
  });

  test("畸形字节不抛异常", () => {
    const samples: number[][] = [
      [],
      [0x6f],
      [0x6f, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00],
      [0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 0],
      [0x50],
      [0x62, 0x00, 0, 0, 0, 0, 0, 0, 0, 0],
      [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
    ];
    for (const bytes of samples) {
      const messages = decodeCcidMessages(context([exchange(1, "out", bytes)]));
      for (const message of messages) expect(Array.isArray(message.anomalies)).toBe(true);
    }
  });
});
