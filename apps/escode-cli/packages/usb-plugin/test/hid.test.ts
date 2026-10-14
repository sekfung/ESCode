import { describe, expect, test } from "vitest";
import {
  decodeHidMessages,
  parseHidReportDescriptor,
  parseHidReportDescriptorDetailed,
  type HidField,
  type HidReportLayout,
} from "../src/decode/hid.js";
import { USB_CLASS, type DecodeContext, type DeviceInterface, type DeviceSummary, type UsbExchange } from "../src/types.js";

/**
 * 用例里的描述符都按 HID 1.11 手算字节：短条目前缀 = (bTag << 4) | (bType << 2) | bSize，
 * bType 0 主 / 1 全局 / 2 局部，bSize 3 表示 4 字节数据。
 */

/** boot 键盘：Generic Desktop/Keyboard, Collection(Application)，modifier 位图 + 保留字节 + 6 按键码 + LED 输出。 */
const BOOT_KEYBOARD_DESCRIPTOR = Uint8Array.from([
  0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, // Usage Page (Generic Desktop), Usage (Keyboard), Collection (Application)
  0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, // Usage Page (Keyboard), Usage Minimum 0xE0, Usage Maximum 0xE7
  0x15, 0x00, 0x25, 0x01, // Logical Minimum 0, Logical Maximum 1
  0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // Report Size 1, Report Count 8, Input (Data,Var,Abs)
  0x95, 0x01, 0x75, 0x08, 0x81, 0x03, // Report Count 1, Report Size 8, Input (Const,Var,Abs) → 1 字节保留
  0x05, 0x07, 0x19, 0x00, 0x29, 0x65, // Usage Page (Keyboard), Usage Minimum 0, Usage Maximum 101
  0x15, 0x00, 0x25, 0x65, // Logical Minimum 0, Logical Maximum 101
  0x75, 0x08, 0x95, 0x06, 0x81, 0x00, // Report Size 8, Report Count 6, Input (Data,Array,Abs)
  0x05, 0x08, 0x19, 0x01, 0x29, 0x05, // Usage Page (LED), Usage Minimum 1, Usage Maximum 5
  0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x05, 0x91, 0x02, // Logical 0..1, Report Size 1, Count 5, Output (Data,Var,Abs)
  0x95, 0x01, 0x75, 0x03, 0x91, 0x03, // Report Count 1, Report Size 3, Output (Const,Var,Abs)
  0xc0, // End Collection
]);

/** 鼠标：Report ID 1 = 3 按钮 + 5 位补位 + X/Y（-127..127），Report ID 2 = 滚轮。 */
const MOUSE_DESCRIPTOR = Uint8Array.from([
  0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, // Usage Page (Generic Desktop), Usage (Mouse), Collection (Application)
  0x85, 0x01, // Report ID 1
  0x09, 0x01, 0xa1, 0x00, // Usage (Pointer), Collection (Physical)
  0x05, 0x09, 0x19, 0x01, 0x29, 0x03, // Usage Page (Button), Usage Minimum 1, Usage Maximum 3
  0x15, 0x00, 0x25, 0x01, // Logical Minimum 0, Logical Maximum 1
  0x95, 0x03, 0x75, 0x01, 0x81, 0x02, // Report Count 3, Report Size 1, Input (Data,Var,Abs)
  0x95, 0x01, 0x75, 0x05, 0x81, 0x03, // Report Count 1, Report Size 5, Input (Const,Var,Abs)
  0x05, 0x01, 0x09, 0x30, 0x09, 0x31, // Usage Page (Generic Desktop), Usage (X), Usage (Y)
  0x15, 0x81, 0x25, 0x7f, // Logical Minimum -127, Logical Maximum 127
  0x75, 0x08, 0x95, 0x02, 0x81, 0x06, // Report Size 8, Report Count 2, Input (Data,Var,Rel)
  0xc0, // End Collection (Physical)
  0x85, 0x02, // Report ID 2
  0x09, 0x38, 0x95, 0x01, 0x75, 0x08, 0x81, 0x06, // Usage (Wheel), Count 1, Size 8, Input (Data,Var,Rel)
  0xc0, // End Collection (Application)
]);

/** Push/Pop：Push 前 16 位 X/Y/Z，Push 后 8 个 1 位按钮，Pop 后 report size/page 应恢复成 16 位/Generic Desktop。 */
const PUSH_POP_DESCRIPTOR = Uint8Array.from([
  0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, // Usage Page (Generic Desktop), Usage X/Y/Z
  0x15, 0x00, 0x26, 0xff, 0x03, // Logical Minimum 0, Logical Maximum 1023
  0x75, 0x10, 0x95, 0x03, 0x81, 0x02, // Report Size 16, Report Count 3, Input (Data,Var,Abs)
  0xa4, // Push
  0x05, 0x09, 0x19, 0x01, 0x29, 0x08, // Usage Page (Button), Usage Minimum 1, Usage Maximum 8
  0x15, 0x00, 0x25, 0x01, // Logical Minimum 0, Logical Maximum 1
  0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // Report Size 1, Report Count 8, Input (Data,Var,Abs)
  0xb4, // Pop
  0x95, 0x01, 0x09, 0x38, 0x81, 0x06, // Report Count 1, Usage (Wheel), Input (Data,Var,Rel)
]);

const HID_INTERFACE: DeviceInterface = { number: 0, classCode: USB_CLASS.hid, subClassCode: 1, protocolCode: 1, protocols: ["hid"] };

function device(overrides: Partial<DeviceSummary> = {}): DeviceSummary {
  return { busDev: "1-4", bus: 1, device: 4, interfaces: [HID_INTERFACE], descriptorSource: "capture", ...overrides };
}

function exchange(overrides: Partial<UsbExchange> & { index: number }): UsbExchange {
  return {
    ts: overrides.index * 100,
    bus: 1,
    device: 4,
    busDev: "1-4",
    endpoint: 0x81,
    direction: "in",
    type: "interrupt",
    data: new Uint8Array(),
    status: 0,
    ...overrides,
  };
}

/** 控制传输 GET_DESCRIPTOR(0x22) 的数据阶段：bmRequestType 0x81、bRequest 0x06、wValue 高字节 0x22。 */
function descriptorExchange(index: number, descriptor: Uint8Array, iface = 0): UsbExchange {
  return exchange({
    index,
    type: "control",
    direction: "in",
    endpoint: 0x80,
    setup: { bmRequestType: 0x81, bRequest: 0x06, wValue: 0x2200, wIndex: iface, wLength: descriptor.length },
    data: descriptor,
  });
}

function context(dev: DeviceSummary, exchanges: UsbExchange[]): DecodeContext {
  return { device: dev, exchanges: [...exchanges].sort((left, right) => left.ts - right.ts) };
}

function layoutFor(layouts: HidReportLayout[], reportId: number): HidReportLayout {
  const layout = layouts.find((item) => item.reportId === reportId);
  if (layout === undefined) throw new Error(`没有 reportId ${reportId} 的布局`);
  return layout;
}

function fieldAt(layout: HidReportLayout, index: number): HidField {
  const field = layout.fields[index];
  if (field === undefined) throw new Error(`没有第 ${index} 个字段`);
  return field;
}

describe("parseHidReportDescriptor", () => {
  test("boot 键盘描述符：modifier 位图、保留字节、数组按键字段与 LED 输出", () => {
    const result = parseHidReportDescriptorDetailed(BOOT_KEYBOARD_DESCRIPTOR);
    expect(result.anomalies).toEqual([]);
    expect(result.unknownItems).toEqual([]);
    expect(result.layouts).toHaveLength(1);
    const layout = layoutFor(result.layouts, 0);
    expect(layout.fields).toHaveLength(16);

    expect(fieldAt(layout, 0)).toEqual({
      bitOffset: 0,
      bitSize: 1,
      usagePage: 0x07,
      usages: [0xe0],
      signed: false,
      flags: { constant: false, variable: true, relative: false },
    });
    expect(fieldAt(layout, 7)).toMatchObject({ bitOffset: 7, usages: [0xe7] });
    // 1 字节保留：Constant|Variable 合成一个 padding 字段
    expect(fieldAt(layout, 8)).toMatchObject({ bitOffset: 8, bitSize: 8, usages: [], flags: { constant: true, variable: true } });

    const keys = fieldAt(layout, 9);
    expect(keys).toMatchObject({ bitOffset: 16, bitSize: 48, usagePage: 0x07, flags: { constant: false, variable: false, relative: false } });
    expect(keys.usages).toHaveLength(102); // Usage Minimum 0..Maximum 101 展开
    expect(keys.usages[0]).toBe(0x00);
    expect(keys.usages[101]).toBe(0x65);

    // LED 输出与输入共用 reportId 位偏移累加器，但不计入输入流长度
    expect(fieldAt(layout, 10)).toMatchObject({ bitOffset: 64, bitSize: 1, usagePage: 0x08, usages: [0x01] });
    expect(fieldAt(layout, 15)).toMatchObject({ bitOffset: 69, bitSize: 3, flags: { constant: true } });
    expect(result.inputBits).toEqual([{ reportId: 0, bits: 64 }]);
    expect(result.arrayElements).toEqual([{ reportId: 0, bitOffset: 16, elementBits: 8 }]);
  });

  test("鼠标描述符（含 Report ID）：两个 reportId 的布局、按钮位图、X/Y 有符号", () => {
    const result = parseHidReportDescriptorDetailed(MOUSE_DESCRIPTOR);
    expect(result.anomalies).toEqual([]);
    expect(result.layouts.map((layout) => layout.reportId)).toEqual([1, 2]);

    const report1 = layoutFor(result.layouts, 1);
    expect(report1.fields).toHaveLength(6);
    expect(fieldAt(report1, 0)).toEqual({
      bitOffset: 0,
      bitSize: 1,
      usagePage: 0x09,
      usages: [1],
      signed: false,
      flags: { constant: false, variable: true, relative: false },
    });
    expect(fieldAt(report1, 2)).toMatchObject({ bitOffset: 2, usages: [3] });
    expect(fieldAt(report1, 3)).toMatchObject({ bitOffset: 3, bitSize: 5, flags: { constant: true, variable: true } });
    expect(fieldAt(report1, 4)).toMatchObject({
      bitOffset: 8,
      bitSize: 8,
      usagePage: 0x01,
      usages: [0x30],
      signed: true, // Logical Minimum -127
      flags: { constant: false, variable: true, relative: true },
    });
    expect(fieldAt(report1, 5)).toMatchObject({ bitOffset: 16, usages: [0x31], signed: true });
    expect(report1.reportSize).toBe(8);
    expect(report1.reportCount).toBe(2);

    const report2 = layoutFor(result.layouts, 2);
    expect(report2.fields).toHaveLength(1);
    expect(fieldAt(report2, 0)).toMatchObject({ bitOffset: 0, bitSize: 8, usagePage: 0x01, usages: [0x38], signed: true, flags: { relative: true } });
    expect(result.inputBits).toEqual([
      { reportId: 1, bits: 24 },
      { reportId: 2, bits: 8 },
    ]);
  });

  test("Usage Minimum..Maximum 展开与 Push/Pop 全局状态栈", () => {
    const result = parseHidReportDescriptorDetailed(PUSH_POP_DESCRIPTOR);
    expect(result.anomalies).toEqual([]);
    const layout = layoutFor(result.layouts, 0);
    expect(layout.fields).toHaveLength(12);

    expect(fieldAt(layout, 0)).toMatchObject({ bitOffset: 0, bitSize: 16, usagePage: 0x01, usages: [0x30], signed: false });
    expect(fieldAt(layout, 2)).toMatchObject({ bitOffset: 32, bitSize: 16, usages: [0x32] });
    // 1..8 连续展开成 8 个按钮字段
    expect(layout.fields.slice(3, 11).map((field) => field.usages[0])).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
    expect(fieldAt(layout, 3)).toMatchObject({ bitOffset: 48, bitSize: 1, usagePage: 0x09 });
    // Pop 恢复 usage page 与 report size：Wheel 又是 16 位、page 回到 Generic Desktop
    expect(fieldAt(layout, 11)).toMatchObject({ bitOffset: 56, bitSize: 16, usagePage: 0x01, usages: [0x38], flags: { relative: true } });
  });

  test("Unit / Unit Exponent 等未覆盖条目只登记，不影响后续字段", () => {
    const descriptor = Uint8Array.from([
      0x05, 0x01, // Usage Page (Generic Desktop)
      0x65, 0x00, // Unit (None) → 未解析
      0x55, 0x0e, // Unit Exponent → 未解析
      0x09, 0x30, // Usage (X)
      0x15, 0x81, 0x25, 0x7f, // Logical Minimum -127, Logical Maximum 127
      0x75, 0x08, 0x95, 0x01, 0x81, 0x02, // Report Size 8, Report Count 1, Input (Data,Var,Abs)
    ]);
    const result = parseHidReportDescriptorDetailed(descriptor);
    expect(result.anomalies).toEqual([]);
    expect(result.unknownItems).toHaveLength(2);
    expect(result.unknownItems.join(" ")).toContain("0x05");
    expect(result.unknownItems.join(" ")).toContain("0x06");
    expect(fieldAt(layoutFor(result.layouts, 0), 0)).toMatchObject({ bitOffset: 0, bitSize: 8, usagePage: 0x01, usages: [0x30], signed: true });
  });

  test("截断/畸形描述符不抛异常，只给 anomaly", () => {
    const truncated = parseHidReportDescriptorDetailed(Uint8Array.from([0x05]));
    expect(truncated.layouts).toEqual([]);
    expect(truncated.anomalies.join("\n")).toContain("截断");

    // 长条目声明 16 字节数据但被打断
    const longItem = parseHidReportDescriptorDetailed(Uint8Array.from([0xfe, 0x10, 0x00]));
    expect(longItem.anomalies.join("\n")).toContain("长条目截断");

    // Report Count = 0xFFFFFFFF（4 字节数据）→ 超限跳过，不去分配内存
    const huge = parseHidReportDescriptorDetailed(Uint8Array.from([0x05, 0x01, 0x09, 0x30, 0x75, 0x08, 0x97, 0xff, 0xff, 0xff, 0xff, 0x81, 0x02]));
    expect(huge.layouts).toEqual([]);
    expect(huge.anomalies.join("\n")).toContain("超过上限");

    // 没有 Collection 的 End Collection
    const unbalanced = parseHidReportDescriptorDetailed(Uint8Array.from([0xc0]));
    expect(unbalanced.anomalies.join("\n")).toContain("End Collection");

    const garbage = new Uint8Array(256);
    let seed = 20240607;
    for (let index = 0; index < garbage.length; index += 1) {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      garbage[index] = seed % 256;
    }
    expect(() => parseHidReportDescriptor(garbage)).not.toThrow();
  });
});

describe("decodeHidMessages", () => {
  test("按描述符布局解码输入报告：padding、modifier 位图与按键数组", () => {
    const messages = decodeHidMessages(
      context(device(), [
        descriptorExchange(1, BOOT_KEYBOARD_DESCRIPTOR),
        exchange({ index: 2, data: Uint8Array.from([0x02, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00]) }),
      ]),
    );
    expect(messages).toHaveLength(1);
    const report = messages[0]!;
    expect(report.kind).toBe("Input report");
    expect(report.anomalies).toEqual([]);
    expect(report.summary).toBe("len=8: Left Shift=1 page 0x07/usage 0x00..0x65=[0x04 0x05]");
    const detail = report.detail.join("\n");
    expect(detail).toContain("bit 1..1  Left Shift = 1");
    expect(detail).toContain("bit 8..15  padding (8 bits)");
    expect(detail).toContain("bit 16..63  page 0x07/usage 0x00..0x65 = [0x04 0x05]");
  });

  test("按 reportId 选布局并解出有符号负值", () => {
    const messages = decodeHidMessages(
      context(device(), [
        descriptorExchange(1, MOUSE_DESCRIPTOR),
        exchange({ index: 2, data: Uint8Array.from([0x01, 0x05, 0xf8, 0x0a]) }),
        exchange({ index: 3, data: Uint8Array.from([0x02, 0xf0]) }),
      ]),
    );
    expect(messages).toHaveLength(2);
    expect(messages[0]!.summary).toBe("ID 0x01 len=4: Button 1=1 Button 3=1 X=-8 Y=10");
    expect(messages[0]!.anomalies).toEqual([]);
    expect(messages[0]!.detail.join("\n")).toContain("bit 8..15  X = -8 (relative)");
    expect(messages[1]!.summary).toBe("ID 0x02 len=2: Wheel=-16");
  });

  test("报告长度与布局不符：计 anomaly 且仍解出能对齐的字段", () => {
    const messages = decodeHidMessages(
      context(device(), [
        descriptorExchange(1, MOUSE_DESCRIPTOR),
        exchange({ index: 2, data: Uint8Array.from([0x01, 0x05, 0xf8]) }),
      ]),
    );
    const report = messages[0]!;
    expect(report.anomalies.join("\n")).toContain("不符（少 1 字节）");
    expect(report.summary).toContain("Button 1=1");
    expect(report.summary).toContain("X=-8");
    expect(report.anomalies.join("\n")).toContain("截断");
  });

  test("描述符缺失：只给 ReportID 与长度，并提示用户在插拔期间重抓", () => {
    const messages = decodeHidMessages(context(device(), [exchange({ index: 1, data: Uint8Array.from([0x02, 0x10, 0x20, 0x30, 0x40]) })]));
    expect(messages).toHaveLength(1);
    const report = messages[0]!;
    expect(report.kind).toBe("Input report");
    expect(report.summary).toBe("ID 0x02 len=5");
    expect(report.detail.length).toBeLessThanOrEqual(4);
    const detail = report.detail.join("\n");
    expect(detail).toContain("报告描述符不在抓包中");
    expect(detail).toContain("无法按字段解码");
    expect(detail).toContain("插拔设备期间抓");
    expect(report.anomalies.join("\n")).toContain("报告描述符不在抓包中");
    expect(report.payload).toEqual(Uint8Array.from([0x02, 0x10, 0x20, 0x30, 0x40]));
  });

  test("描述符缺失但长度是 boot 固定格式：按 boot 键盘/鼠标解码", () => {
    const messages = decodeHidMessages(
      context(device(), [
        exchange({ index: 1, data: Uint8Array.from([0x02, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00]) }),
        exchange({ index: 2, data: Uint8Array.from([0x01, 0xf8, 0x0a]) }),
      ]),
    );
    expect(messages[0]!.summary).toBe("Boot keyboard len=8: Left Shift Keys[0x04 0x05]");
    expect(messages[0]!.detail.join("\n")).toContain("报告描述符不在抓包中");
    expect(messages[0]!.detail.join("\n")).toContain("page 0x07/usage 0x04");
    expect(messages[1]!.summary).toBe("Boot mouse len=3: Buttons 0x01(Left) X=-8 Y=10");
  });

  test("接口处于 boot 协议时优先按 boot 固定格式解码", () => {
    const bootDevice = device({ interfaces: [{ ...HID_INTERFACE, protocolCode: 0 }] });
    const messages = decodeHidMessages(
      context(bootDevice, [
        descriptorExchange(1, MOUSE_DESCRIPTOR),
        exchange({ index: 2, data: Uint8Array.from([0x02, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00]) }),
      ]),
    );
    expect(messages[0]!.summary).toBe("Boot keyboard len=8: Left Shift Keys[0x04 0x05]");
    expect(messages[0]!.detail.join("\n")).toContain("boot 协议");
  });

  test("SET_IDLE / SET_PROTOCOL / GET_REPORT / SET_REPORT 产出 Control request 消息", () => {
    const messages = decodeHidMessages(
      context(device(), [
        exchange({ index: 1, type: "control", direction: "out", endpoint: 0x00, setup: { bmRequestType: 0x21, bRequest: 0x0a, wValue: 0x7d00, wIndex: 0, wLength: 0 } }),
        exchange({ index: 2, type: "control", direction: "out", endpoint: 0x00, setup: { bmRequestType: 0x21, bRequest: 0x0b, wValue: 0x0000, wIndex: 0, wLength: 0 } }),
        exchange({
          index: 3,
          type: "control",
          direction: "out",
          endpoint: 0x00,
          setup: { bmRequestType: 0x21, bRequest: 0x09, wValue: 0x0201, wIndex: 0, wLength: 4 },
          data: Uint8Array.from([0x05, 0xf8, 0x0a, 0x00]),
        }),
        exchange({
          index: 4,
          type: "control",
          direction: "in",
          endpoint: 0x80,
          setup: { bmRequestType: 0xa1, bRequest: 0x01, wValue: 0x0100, wIndex: 0, wLength: 8 },
          data: Uint8Array.from([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        }),
      ]),
    );
    expect(messages).toHaveLength(4);
    expect(messages.every((message) => message.kind === "Control request")).toBe(true);
    expect(messages[0]!.summary).toBe("SET_IDLE iface=0 duration=500ms reportID=0x00");
    expect(messages[1]!.summary).toBe("SET_PROTOCOL iface=0 protocol=boot");
    expect(messages[2]!.summary).toBe("SET_REPORT iface=0 type=output reportID=0x01 len=4");
    expect(messages[3]!.summary).toBe("GET_REPORT iface=0 type=input reportID=0x00 len=8");
  });

  test("非 HID 设备不产出消息", () => {
    const msc = device({
      interfaces: [{ number: 0, classCode: USB_CLASS.massStorage, subClassCode: 0x06, protocolCode: 0x50, protocols: ["msc"] }],
    });
    expect(decodeHidMessages(context(msc, [exchange({ index: 1, data: Uint8Array.from([1, 2, 3, 4]) })]))).toEqual([]);
  });

  test("描述符里的未解析 tag 只在该接口第一条报告里提示一次", () => {
    // 同一份描述符里带 Unit（tag 0x6）与 Unit Exponent（tag 0x5）：解析器只登记不猜。
    const descriptor = Uint8Array.from([
      0x05, 0x01, 0x65, 0x00, 0x55, 0x0e, 0x09, 0x30, // Usage Page, Unit, Unit Exponent, Usage (X)
      0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x02, // Logical -127..127, Size 8, Count 1, Input
    ]);
    const messages = decodeHidMessages(
      context(device(), [
        descriptorExchange(1, descriptor),
        exchange({ index: 2, data: Uint8Array.from([0x05]) }),
        exchange({ index: 3, data: Uint8Array.from([0x06]) }),
      ]),
    );
    expect(messages).toHaveLength(2);
    expect(messages[0]!.detail.join("\n")).toContain("未解析条目");
    expect(messages[1]!.detail.join("\n")).not.toContain("未解析条目");
  });
});
