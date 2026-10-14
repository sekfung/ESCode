/**
 * MSC / SCSI（BOT）解码的字节级用例：CBW/CDB/CSW、INQUIRY 与 REQUEST SENSE 响应
 * 全部按 SCSI/SPC 规范手工构造，不为了让断言通过而迁就实现。
 */

import { describe, expect, test } from "vitest";
import { decodeMscMessages } from "../src/decode/msc.js";
import type { DecodeContext, DecodedMessage, DeviceSummary, UsbExchange } from "../src/types.js";

const CBW_SIGNATURE = 0x43425355;
const CSW_SIGNATURE = 0x53425355;

/** 小端 32 位字节。 */
function le32(value: number): number[] {
  return [value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff];
}

let exchangeCursor = 0;

function makeExchange(partial: Partial<UsbExchange> & { data: Uint8Array }): UsbExchange {
  exchangeCursor += 1;
  return {
    index: exchangeCursor,
    ts: exchangeCursor * 10,
    bus: 1,
    device: 4,
    busDev: "1-4",
    endpoint: 0x81,
    direction: "in",
    type: "bulk",
    status: 0,
    ...partial,
  };
}

/** CBW：31 字节定长，CDB 不足 16 字节时补零。 */
function cbwPacket(options: {
  tag: number;
  transferLength: number;
  cdb: number[];
  flags?: number;
}): Uint8Array {
  return Uint8Array.from([
    ...le32(CBW_SIGNATURE),
    ...le32(options.tag),
    ...le32(options.transferLength),
    options.flags ?? 0x80, // bmCBWFlags: bit7 = 数据阶段 IN
    0x00, // bCBWLUN
    options.cdb.length, // bCBWCBLength
    ...options.cdb,
    ...Array.from({ length: 16 - options.cdb.length }, () => 0x00),
  ]);
}

/** CSW：13 字节定长。 */
function cswPacket(options: { tag: number; residue: number; status: number }): Uint8Array {
  return Uint8Array.from([
    ...le32(CSW_SIGNATURE),
    ...le32(options.tag),
    ...le32(options.residue),
    options.status,
  ]);
}

/** READ(10)：LBA 在字节 2-5（大端），块数在字节 7-8（大端）。 */
const READ10_CDB = [0x28, 0x00, 0x00, 0x00, 0x12, 0x34, 0x00, 0x00, 0x08, 0x00];

function paddedAscii(text: string, length: number): number[] {
  const bytes = Array.from<number>({ length }).fill(0x20);
  for (let index = 0; index < text.length && index < length; index += 1) {
    bytes[index] = text.charCodeAt(index);
  }
  return bytes;
}

/** INQUIRY 响应 36 字节：直接存取设备 + vendor/product/revision。 */
const INQUIRY_RESPONSE = Uint8Array.from([
  0x00, 0x00, 0x05, 0x02, 0x1f, 0x00, 0x00, 0x02,
  ...paddedAscii("ACME", 8),
  ...paddedAscii("FLASH-DISK", 16),
  ...paddedAscii("1.00", 4),
]);

/** 固定格式 sense data：0x70 当前错误，sense key=3（MEDIUM ERROR），ASC/ASCQ=0x11/0x00。 */
const SENSE_MEDIUM_ERROR = Uint8Array.from([
  0x70, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00,
  0x00, 0x00, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

const MSC_DEVICE: DeviceSummary = {
  busDev: "1-4",
  bus: 1,
  device: 4,
  interfaces: [
    { number: 0, classCode: 0x08, subClassCode: 0x06, protocolCode: 0x50, protocols: ["msc"] },
  ],
  descriptorSource: "capture",
};

function contextOf(exchanges: UsbExchange[], device: DeviceSummary = MSC_DEVICE): DecodeContext {
  return { exchanges, device };
}

function first(items: DecodedMessage[]): DecodedMessage {
  const [head] = items;
  if (head === undefined) throw new Error("期望至少产出一条消息");
  return head;
}

describe("MSC / BOT 解码", () => {
  test("CBW(READ(10)) + 数据阶段 + CSW 成功合成一条逻辑命令", () => {
    const tag = 0x0000abcd;
    const cbw = makeExchange({
      data: cbwPacket({ tag, transferLength: 4096, cdb: READ10_CDB }),
      direction: "out",
      endpoint: 0x02,
    });
    const data = makeExchange({ data: new Uint8Array(4096) });
    const csw = makeExchange({ data: cswPacket({ tag, residue: 0, status: 0x00 }) });

    const messages = decodeMscMessages(contextOf([cbw, data, csw]));

    expect(messages).toHaveLength(1);
    const message = first(messages);
    expect(message.kind).toBe("READ(10)");
    expect(message.summary).toContain("READ(10)");
    expect(message.summary).toContain("LBA=0x1234");
    expect(message.summary).toContain("len=8 块");
    expect(message.summary).toContain("CSW 成功");
    expect(message.anomalies).toEqual([]);
    expect(message.exchanges).toEqual([cbw.index, data.index, csw.index]);
    expect(message.busDev).toBe("1-4");
    expect(message.ts).toBe(cbw.ts);
  });

  test("CSW 失败 + REQUEST SENSE：sense key 计入异常", () => {
    const failedTag = 0x00000011;
    // WRITE(10) LBA=0x10 1 块：数据阶段没开始就被拒绝，CSW 残差 512、状态 0x01。
    const failed = [
      makeExchange({
        data: cbwPacket({
          tag: failedTag,
          transferLength: 512,
          cdb: [0x2a, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x01, 0x00],
          flags: 0x00,
        }),
        direction: "out",
        endpoint: 0x02,
      }),
      makeExchange({ data: cswPacket({ tag: failedTag, residue: 512, status: 0x01 }) }),
    ];
    const senseTag = 0x00000012;
    const requestSense = [
      makeExchange({
        data: cbwPacket({ tag: senseTag, transferLength: 18, cdb: [0x03, 0x00, 0x00, 0x00, 0x12, 0x00] }),
        direction: "out",
        endpoint: 0x02,
      }),
      makeExchange({ data: SENSE_MEDIUM_ERROR }),
      makeExchange({ data: cswPacket({ tag: senseTag, residue: 0, status: 0x00 }) }),
    ];

    const messages = decodeMscMessages(contextOf([...failed, ...requestSense]));

    expect(messages).toHaveLength(2);
    const command = first(messages);
    expect(command.kind).toBe("WRITE(10)");
    expect(command.anomalies.some((item) => item.includes("CSW 状态 0x01"))).toBe(true);
    expect(command.anomalies.some((item) => item.includes("MEDIUM ERROR"))).toBe(true);
    expect(command.anomalies.some((item) => item.includes("0x11/0x00") && item.includes("未恢复读错误"))).toBe(true);

    const senseMessage = messages[1];
    expect(senseMessage?.kind).toBe("REQUEST SENSE");
    expect(senseMessage?.detail.some((line) => line.includes("MEDIUM ERROR"))).toBe(true);
    expect(senseMessage?.detail.some((line) => line.includes("0x11/0x00") && line.includes("未恢复读错误"))).toBe(true);
    expect(senseMessage?.summary).toContain("MEDIUM ERROR 0x11/0x00 未恢复读错误");
  });

  test("CSW tag 与 CBW 不一致：计入异常且不崩", () => {
    const messages = decodeMscMessages(
      contextOf([
        makeExchange({
          data: cbwPacket({ tag: 0x1111, transferLength: 0, cdb: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00] }),
          direction: "out",
          endpoint: 0x02,
        }),
        makeExchange({ data: cswPacket({ tag: 0x2222, residue: 0, status: 0x00 }) }),
      ]),
    );

    expect(messages).toHaveLength(1);
    const message = first(messages);
    expect(message.kind).toBe("TEST UNIT READY");
    expect(message.anomalies.some((item) => item.includes("tag=") && item.includes("不一致"))).toBe(true);
  });

  test("短传输：CBW 声明、实际数据与 CSW 残差不符时计入异常", () => {
    const tag = 0x00000033;
    const messages = decodeMscMessages(
      contextOf([
        makeExchange({
          data: cbwPacket({ tag, transferLength: 4096, cdb: READ10_CDB }),
          direction: "out",
          endpoint: 0x02,
        }),
        makeExchange({ data: new Uint8Array(2048) }),
        makeExchange({ data: cswPacket({ tag, residue: 2048, status: 0x00 }) }),
      ]),
    );

    const message = first(messages);
    expect(message.anomalies.some((item) => item.includes("短传输"))).toBe(true);
    expect(message.anomalies.some((item) => item.includes("2048") && item.includes("残差"))).toBe(true);

    // 残差与数据结构不一致时也要报出来（CSW 说残差 1024，但实际少了 2048）
    const inconsistent = decodeMscMessages(
      contextOf([
        makeExchange({
          data: cbwPacket({ tag, transferLength: 4096, cdb: READ10_CDB }),
          direction: "out",
          endpoint: 0x02,
        }),
        makeExchange({ data: new Uint8Array(2048) }),
        makeExchange({ data: cswPacket({ tag, residue: 1024, status: 0x00 }) }),
      ]),
    );
    expect(first(inconsistent).anomalies.some((item) => item.includes("不一致"))).toBe(true);
  });

  test("INQUIRY 响应解出外设类型与 vendor/product/revision", () => {
    const tag = 0x00000044;
    const messages = decodeMscMessages(
      contextOf([
        makeExchange({
          data: cbwPacket({ tag, transferLength: 36, cdb: [0x12, 0x00, 0x00, 0x00, 0x24, 0x00] }),
          direction: "out",
          endpoint: 0x02,
        }),
        makeExchange({ data: INQUIRY_RESPONSE }),
        makeExchange({ data: cswPacket({ tag, residue: 0, status: 0x00 }) }),
      ]),
    );

    const message = first(messages);
    expect(message.kind).toBe("INQUIRY");
    expect(message.summary).toContain("数据 36 字节");
    expect(message.detail.some((line) => line.includes("直接存取设备"))).toBe(true);
    expect(message.detail.some((line) => line.includes("INQUIRY vendor: ACME"))).toBe(true);
    expect(message.detail.some((line) => line.includes("INQUIRY product: FLASH-DISK"))).toBe(true);
    expect(message.detail.some((line) => line.includes("INQUIRY revision: 1.00"))).toBe(true);
    expect(message.anomalies).toEqual([]);
  });

  test("未知操作码与截断 CBW：给异常且不抛", () => {
    const tag = 0x00000055;
    const unknown = decodeMscMessages(
      contextOf([
        makeExchange({
          data: cbwPacket({ tag, transferLength: 0, cdb: [0x9f, 0x00, 0x00, 0x00] }),
          direction: "out",
          endpoint: 0x02,
        }),
        makeExchange({ data: cswPacket({ tag, residue: 0, status: 0x00 }) }),
      ]),
    );
    const unknownMessage = first(unknown);
    expect(unknownMessage.kind).toBe("unknown(0x9F)");
    expect(unknownMessage.anomalies.some((item) => item.includes("0x9F"))).toBe(true);

    // 20 字节、只有签名可辨的 CBW（小端 0x43425355）
    const truncated = new Uint8Array(20);
    truncated.set([0x55, 0x53, 0x42, 0x43], 0);
    const truncatedMessages = decodeMscMessages(
      contextOf([makeExchange({ data: truncated, direction: "out", endpoint: 0x02 })]),
    );
    expect(truncatedMessages).toHaveLength(1);
    const truncatedMessage = first(truncatedMessages);
    expect(truncatedMessage.kind).toBe("CBW 截断");
    expect(truncatedMessage.anomalies.some((item) => item.includes("长度不足"))).toBe(true);

    expect(() => decodeMscMessages(contextOf([]))).not.toThrow();
    expect(decodeMscMessages(contextOf([]))).toEqual([]);
  });

  test("非 BOT 报文不产出消息：无签名与走控制端点", () => {
    const vendorDevice: DeviceSummary = {
      ...MSC_DEVICE,
      interfaces: [
        { number: 0, classCode: 0xff, subClassCode: 0x00, protocolCode: 0x00, protocols: [] },
      ],
    };
    const plain = decodeMscMessages(
      contextOf([makeExchange({ data: Uint8Array.from([1, 2, 3, 4, 5, 6]) })], vendorDevice),
    );
    expect(plain).toEqual([]);

    // BOT 只用 bulk 端点：控制传输上出现 CBW 签名不是命令
    const control = decodeMscMessages(
      contextOf([
        makeExchange({
          type: "control",
          data: cbwPacket({ tag: 0x66, transferLength: 0, cdb: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00] }),
        }),
      ]),
    );
    expect(control).toEqual([]);
  });

  test("CBW/CSW 签名开头的任意长度字节都能安全处理", () => {
    for (let size = 0; size <= 40; size += 1) {
      for (const signature of [CBW_SIGNATURE, CSW_SIGNATURE]) {
        const data = new Uint8Array(size);
        for (let index = 0; index < 4 && index < size; index += 1) {
          data[index] = (signature >>> (index * 8)) & 0xff;
        }
        expect(() => decodeMscMessages(contextOf([makeExchange({ data })], MSC_DEVICE))).not.toThrow();
      }
    }
  });

  test("同一设备多条命令按时间顺序各成一条，缺 CSW 时给异常", () => {
    const readyTag = 0x00000071;
    const readTag = 0x00000072;
    const messages = decodeMscMessages(
      contextOf([
        makeExchange({
          data: cbwPacket({ tag: readyTag, transferLength: 0, cdb: [0x00, 0x00, 0x00, 0x00, 0x00, 0x00] }),
          direction: "out",
          endpoint: 0x02,
        }),
        makeExchange({ data: cswPacket({ tag: readyTag, residue: 0, status: 0x00 }) }),
        makeExchange({
          data: cbwPacket({ tag: readTag, transferLength: 4096, cdb: READ10_CDB }),
          direction: "out",
          endpoint: 0x02,
        }),
        makeExchange({ data: new Uint8Array(4096) }),
        // 缺 CSW
      ]),
    );

    expect(messages.map((message) => message.kind)).toEqual(["TEST UNIT READY", "READ(10)"]);
    expect(messages[0]?.anomalies).toEqual([]);
    expect(messages[1]?.anomalies.some((item) => item.includes("没有收到 CSW"))).toBe(true);
  });
});
