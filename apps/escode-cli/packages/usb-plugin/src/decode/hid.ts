/**
 * HID 报告描述符解析与报告解码（docs/specs/usb-agent-tools.md §7）。
 * 流程：控制 GET_DESCRIPTOR(0x22) → 按接口（wIndex 低字节）分组的布局表；中断 IN 数据 → 按 reportId 选布局 →
 * 位域切片；描述符不在抓包里时只给 ReportID 与长度（8 字节键盘 / 3-4 字节鼠标按 boot 固定格式兜底）并提示重抓。
 * 只消费 HID 1.11 有明确语义的条目：Unit 类与设计符/字符串索引只登记不猜；截断/畸形/超限降级成 anomaly，不抛异常。
 * 越界读走 format.ts 的助手，报告正文 hex 走 toHex（§8 体积上限）。
 */
import { byteAt, hex, readBytesLE, setBitNames, toHex } from "../format.js";
import { USB_CLASS, type DecodeContext, type DecodedMessage, type DeviceInterface, type UsbExchange, type UsbSetupPacket } from "../types.js";

export interface HidField {
  /** bit 偏移（相对该 reportId 的报告正文起点，含 reportId 字节后）。 */
  bitOffset: number;
  bitSize: number;
  usagePage: number;
  usages: number[];
  /** 变量字段的有符号解释（logical minimum 为负时为 true）。 */
  signed: boolean;
  flags: { constant: boolean; variable: boolean; relative: boolean };
}

export interface HidReportLayout {
  reportId: number;
  /** 最近一次生成字段的主条目参数（布局只有 reportId 一维，不区分输入/输出流）。 */
  reportSize: number;
  reportCount: number;
  fields: HidField[];
}

/** 解析产物；`parseHidReportDescriptor` 只暴露 `layouts`。 */
export interface HidDescriptorParseResult {
  layouts: HidReportLayout[];
  /** 截断、畸形、超限说明与未解析条目（Unit 类、保留 tag），不猜语义。 */
  anomalies: string[];
  unknownItems: string[];
  /** 数组字段元素位宽：公开 HidField 没有这一维，按 (reportId, bitOffset) 查。 */
  arrayElements: { reportId: number; bitOffset: number; elementBits: number }[];
  /** Input 主条目在每个 reportId 下的位长：长度校验只看输入流。 */
  inputBits: { reportId: number; bits: number }[];
}

// 短条目前缀 = bSize(bit0-1，3 表示 4 字节) | bType(bit2-3) | bTag(bit4-7)（HID 1.11 §6.2.2.2）。
const ITEM_TYPE = { main: 0, global: 1, local: 2 };
const MAIN_TAG = { input: 0x8, output: 0x9, collection: 0xa, feature: 0xb, endCollection: 0xc };
const GLOBAL_TAG = { usagePage: 0, logicalMin: 1, logicalMax: 2, physicalMin: 3, physicalMax: 4, reportSize: 7, reportId: 8, reportCount: 9, push: 10, pop: 11 };
const LOCAL_TAG = { usage: 0, usageMin: 1, usageMax: 2 };
const GLOBAL_FIELD: Record<number, "usagePage" | "logicalMin" | "logicalMax" | "reportSize" | "reportId" | "reportCount"> = { 0: "usagePage", 1: "logicalMin", 2: "logicalMax", 7: "reportSize", 8: "reportId", 9: "reportCount" };
// 安全上限与输出自限（§8）：畸形描述符不能让解析结果和视图膨胀。
const LONG_ITEM = 0xfe, MAX_FIELDS_PER_ITEM = 256, MAX_FIELDS_PER_REPORT = 1024, MAX_ITEM_BITS = 1 << 17, MAX_USAGE = 512;
const MAX_STACK = 64, MAX_UNKNOWN = 16, MAX_FIELD_LINES = 24, MAX_VALUES = 8, MAX_SUMMARY = 160;

interface UsageRef { page: number; id: number }
interface GlobalState { usagePage: number; logicalMin: number; logicalMax: number; reportSize: number; reportCount: number; reportId: number }
/** 局部条目只对紧随其后的第一个主条目生效（HID 1.11 §6.2.2.8）。 */
interface LocalState { usages: UsageRef[]; usageMin?: UsageRef; usageMax?: UsageRef }

// 不出现 Report ID 条目时报告里没有 ID 字节，位偏移从报告第 0 字节算起。
const defaultGlobals = (): GlobalState => ({ usagePage: 0, logicalMin: 0, logicalMax: 0, reportSize: 0, reportCount: 0, reportId: 0 });

/** 逻辑最值按字节数补码扩展（Report Size 等无符号条目不走这里）。 */
const signExtend = (value: number, n: number): number => (n >= 1 && n <= 4 && value >= 2 ** (n * 8 - 1) ? value - 2 ** (n * 8) : value);

/** 主条目的 usage 列表：显式 Usage 在前，Usage Minimum..Maximum 展开成连续序列。 */
function expandUsages(locals: LocalState, anomalies: string[]): UsageRef[] {
  const usages = [...locals.usages];
  const min = locals.usageMin;
  const max = locals.usageMax;
  if (min === undefined || max === undefined) return usages;
  if (min.page !== max.page || max.id < min.id) {
    anomalies.push(`Usage Minimum..Maximum 区间无效（${hex(min.id)}..${hex(max.id)}），已忽略`);
    return usages;
  }
  const span = max.id - min.id + 1;
  const take = Math.min(span, MAX_USAGE);
  if (take < span) anomalies.push(`Usage Minimum..Maximum 展开 ${span} 条超过上限，截断到 ${take} 条`);
  for (let id = min.id; id < min.id + take; id += 1) usages.push({ page: min.page, id });
  return usages;
}

export function parseHidReportDescriptorDetailed(descriptor: Uint8Array): HidDescriptorParseResult {
  const byId = new Map<number, HidReportLayout>();
  const offsets = new Map<number, number>();
  const inputBits = new Map<number, number>();
  const arrayElements: HidDescriptorParseResult["arrayElements"] = [];
  const anomalies: string[] = [];
  const unknownItems: string[] = [];
  const seen = new Set<string>();
  const stack: GlobalState[] = [];
  let globals = defaultGlobals();
  let locals: LocalState = { usages: [] };
  let depth = 0;
  let cursor = 0;

  const noteUnknown = (label: string): void => {
    if (seen.has(label) || unknownItems.length >= MAX_UNKNOWN) return;
    seen.add(label);
    unknownItems.push(label);
  };

  /** 主条目 Input/Output/Feature：按全局状态切片，位偏移按 reportId 累加。 */
  const appendFields = (flags: number, isInput: boolean): void => {
    const { reportSize: size, reportCount: count, reportId, usagePage, logicalMin } = globals;
    if (size <= 0 || count <= 0) return; // 0 位/0 个的条目按规范不产生数据
    const totalBits = size * count;
    if (totalBits > MAX_ITEM_BITS) { anomalies.push(`主条目声明 ${totalBits} 位（超过上限），已跳过`); return; }
    let layout = byId.get(reportId);
    if (layout === undefined) { layout = { reportId, reportSize: size, reportCount: count, fields: [] }; byId.set(reportId, layout); }
    layout.reportSize = size;
    layout.reportCount = count;
    const start = offsets.get(reportId) ?? 0;
    let capped = false;
    const push = (field: HidField): void => {
      if (layout.fields.length < MAX_FIELDS_PER_REPORT) layout.fields.push(field);
      else if (!capped) { capped = true; anomalies.push(`reportId ${hex(reportId)} 字段数达到上限，后续字段被忽略`); }
    };
    const fieldFlags = { constant: (flags & 1) !== 0, variable: (flags & 2) !== 0, relative: (flags & 4) !== 0 };
    const signed = logicalMin < 0;
    const usages = expandUsages(locals, anomalies);
    // 常量整段合成一个 padding 字段：逐位展开只会在视图里刷屏。
    if (fieldFlags.constant) push({ bitOffset: start, bitSize: totalBits, usagePage, usages: [], signed: false, flags: fieldFlags });
    else if (fieldFlags.variable) {
      const emit = Math.min(count, MAX_FIELDS_PER_ITEM);
      if (emit < count) anomalies.push(`reportId ${hex(reportId)} 的变量字段 ${count} 个超过单条目上限，只生成前 ${emit} 个`);
      for (let index = 0; index < emit; index += 1) {
        const usage = usages[index] ?? usages[usages.length - 1]; // usage 少于字段数时沿用最后一个
        push({ bitOffset: start + index * size, bitSize: size, usagePage: usage?.page ?? usagePage, usages: usage ? [usage.id] : [], signed, flags: fieldFlags });
      }
    } else {
      // 数组整段一个字段；元素位宽记进侧表供解码时逐元素切片。
      push({ bitOffset: start, bitSize: totalBits, usagePage: usages[0]?.page ?? usagePage, usages: usages.map((usage) => usage.id), signed, flags: fieldFlags });
      arrayElements.push({ reportId, bitOffset: start, elementBits: size });
    }
    const end = start + totalBits;
    offsets.set(reportId, end);
    if (isInput) inputBits.set(reportId, Math.max(inputBits.get(reportId) ?? 0, end));
  };

  while (cursor < descriptor.length) {
    const prefix = byteAt(descriptor, cursor);
    if (prefix === undefined) break;
    const at = cursor;
    cursor += 1;
    if (prefix === LONG_ITEM) {
      // 长条目（0xFE + 1 字节长度 + 数据）规范未定义语义：整体跳过。
      const length = byteAt(descriptor, cursor);
      if (length === undefined || cursor + 1 + length > descriptor.length) { anomalies.push(`描述符在 ${hex(at)} 处的长条目截断`); break; }
      cursor += 1 + length;
      continue;
    }
    const dataSize = prefix & 3;
    const itemType = (prefix >> 2) & 3;
    const tag = prefix >> 4;
    const byteCount = dataSize === 3 ? 4 : dataSize;
    const raw = readBytesLE(descriptor, cursor, byteCount);
    if (raw === undefined) { anomalies.push(`描述符在 ${hex(at)} 处截断（条目声明 ${byteCount} 字节数据）`); break; }
    cursor += byteCount;

    if (itemType === ITEM_TYPE.global) {
      if (tag === GLOBAL_TAG.push) {
        if (stack.length >= MAX_STACK) anomalies.push("Push 超过全局状态栈上限，条目被忽略");
        else stack.push({ ...globals });
      } else if (tag === GLOBAL_TAG.pop) {
        // Pop 早于 Push（描述符畸形）时保留当前状态继续，不抛异常。
        const restored = stack.pop();
        if (restored === undefined) anomalies.push("Pop 时全局状态栈为空（描述符畸形）");
        else globals = restored;
      } else if (tag !== GLOBAL_TAG.physicalMin && tag !== GLOBAL_TAG.physicalMax) {
        // Physical Min/Max 只占位（公开布局里没有物理量程）；Unit 类条目需要单位换算表，不猜。
        const field = GLOBAL_FIELD[tag];
        if (field === undefined) noteUnknown(`未解析的全局条目 tag ${hex(tag)}`);
        else globals[field] = tag === GLOBAL_TAG.logicalMin || tag === GLOBAL_TAG.logicalMax ? signExtend(raw, byteCount) : raw;
      }
      continue;
    }

    if (itemType === ITEM_TYPE.local) {
      const page = globals.usagePage;
      // 4 字节 Usage 是扩展形式：高 16 位为 usage page（HID 1.11 §6.2.2.8）。
      if (tag === LOCAL_TAG.usage) locals.usages.push(dataSize === 4 ? { page: raw >>> 16, id: raw & 0xffff } : { page, id: raw });
      else if (tag === LOCAL_TAG.usageMin) locals.usageMin = { page, id: raw };
      else if (tag === LOCAL_TAG.usageMax) locals.usageMax = { page, id: raw };
      else noteUnknown(`未解析的局部条目 tag ${hex(tag)}`); // Designator / String Index / Delimiter
      continue;
    }

    if (itemType === ITEM_TYPE.main) {
      if (tag === MAIN_TAG.input) appendFields(raw, true);
      else if (tag === MAIN_TAG.output || tag === MAIN_TAG.feature) appendFields(raw, false); // 与输入共用 reportId 累加器
      else if (tag === MAIN_TAG.collection) depth += 1;
      else if (tag === MAIN_TAG.endCollection) {
        depth -= 1;
        if (depth < 0) { anomalies.push("End Collection 多于 Collection（描述符畸形）"); depth = 0; }
      } else noteUnknown(`保留的主条目 tag ${hex(tag)}`);
      locals = { usages: [] }; // 局部条目只对紧随其后的主条目生效
      continue;
    }

    noteUnknown(`保留的条目类型（type=3, tag ${hex(tag)}）`);
  }

  if (depth !== 0) anomalies.push(`Collection 嵌套不平衡（结束时深度 ${depth}）`);
  return { layouts: [...byId.values()], anomalies, unknownItems, arrayElements, inputBits: [...inputBits].map(([reportId, bits]) => ({ reportId, bits })) };
}

/** 报告描述符 → 报告布局（导出以便单测与 `usb_probe` 复用）。 */
export function parseHidReportDescriptor(descriptor: Uint8Array): HidReportLayout[] {
  return parseHidReportDescriptorDetailed(descriptor).layouts;
}

// 只收录有把握的名称；未收录的 usage 写成 `page 0xNN/usage 0xNN`，不按编号猜名字。
const USAGE_NAMES: Record<number, string> = {
  0x0102: "Mouse", 0x0104: "Joystick", 0x0105: "Game Pad", 0x0106: "Keyboard", 0x0108: "Multi-axis",
  0x0130: "X", 0x0131: "Y", 0x0132: "Z", 0x0133: "Rx", 0x0134: "Ry", 0x0135: "Rz", 0x0136: "Slider",
  0x0138: "Wheel", 0x0139: "Hat switch", 0x0700: "No event",
  0x07e0: "Left Ctrl", 0x07e1: "Left Shift", 0x07e2: "Left Alt", 0x07e3: "Left GUI",
  0x07e4: "Right Ctrl", 0x07e5: "Right Shift", 0x07e6: "Right Alt", 0x07e7: "Right GUI",
  0x0801: "NumLock", 0x0802: "CapsLock", 0x0803: "ScrollLock",
  0x0c01: "Consumer Control", 0x0c30: "Power", 0x0cb5: "Next Track", 0x0cb6: "Previous Track",
  0x0ccd: "Play/Pause", 0x0ce2: "Mute", 0x0ce9: "Volume Up", 0x0cea: "Volume Down",
};
const BUTTON_PAGE = 0x09, KEYBOARD_PAGE = 0x07;

function usageName(page: number, id: number | undefined): string {
  if (id === undefined) return `page ${hex(page)}/usage -`;
  if (page === BUTTON_PAGE) return `Button ${id}`;
  return USAGE_NAMES[(page << 16) | id] ?? `page ${hex(page)}/usage ${hex(id)}`;
}

/** 字段名：数组 usage 少时逐个列出，多时只给区间（键盘 0x00..0x65 会撑爆 detail）。 */
function fieldName(field: HidField): string {
  const [first] = field.usages;
  if (first === undefined) return "Array";
  if (field.flags.variable || field.usages.length === 1) return usageName(field.usagePage, first);
  if (field.usages.length <= 4) return field.usages.map((id) => usageName(field.usagePage, id)).join("/");
  return `page ${hex(field.usagePage)}/usage ${hex(first)}..${hex(field.usages[field.usages.length - 1] ?? first)}`;
}

const clampSummary = (text: string): string => (text.length <= MAX_SUMMARY ? text : `${text.slice(0, MAX_SUMMARY - 1)}…`);

/** 单个接口（GET_DESCRIPTOR 0x22 的 wIndex 低字节）的报告描述符解析结果。 */
interface CapturedDescriptor {
  interfaceNumber: number;
  layouts: HidReportLayout[];
  arrayElements: HidDescriptorParseResult["arrayElements"];
  inputBits: HidDescriptorParseResult["inputBits"];
  anomalies: string[];
  unknownItems: string[];
  fieldCount: number;
}
interface HidEnv { descriptors: Map<number, CapturedDescriptor>; hidInterfaces: DeviceInterface[]; usesReportIds: boolean }
interface LayoutPick { captured: CapturedDescriptor; layout: HidReportLayout }

const MISSING_HINT = "报告描述符不在抓包中，无法按字段解码；让用户在复现时抓包（插拔设备期间抓）";
const HID_SET_IDLE = 0x0a, HID_SET_PROTOCOL = 0x0b;

const msg = (x: UsbExchange, kind: string, summary: string, detail: string[], anomalies: string[] = [], payload?: Uint8Array): DecodedMessage => ({
  exchanges: [x.index],
  ts: x.ts,
  busDev: x.busDev,
  direction: x.direction,
  kind,
  summary: clampSummary(summary),
  detail,
  anomalies,
  payload,
});

/** 从报告正文按 LSB-first 取位；越界返回 undefined，调用方据此降级。 */
function readBits(data: Uint8Array, bitOffset: number, bitSize: number): number | undefined {
  if (bitSize <= 0 || bitSize > 53) return undefined; // 超过 2^53 无法精确表示
  let value = 0;
  for (let index = 0; index < bitSize; index += 1) {
    const bit = bitOffset + index;
    const byte = byteAt(data, bit >> 3);
    if (byte === undefined) return undefined;
    if ((byte & (1 << (bit & 7))) !== 0) value += 2 ** index;
  }
  return value;
}

const signExtendBits = (value: number, bits: number): number => (bits >= 1 && bits <= 53 && value >= 2 ** (bits - 1) ? value - 2 ** bits : value);
const signedByte = (value: number): number => (value >= 0x80 ? value - 0x100 : value);
const findLayout = (captured: CapturedDescriptor, reportId: number): HidReportLayout | undefined => captured.layouts.find((layout) => layout.reportId === reportId);

/** 报告字节数：优先用输入流位长（布局表混了输入/输出，按最大位偏移会多算）。 */
function expectedReportBytes(layout: HidReportLayout, inputBits: CapturedDescriptor["inputBits"]): number {
  const bits = inputBits.find((entry) => entry.reportId === layout.reportId)?.bits ?? layout.fields.reduce((max, f) => Math.max(max, f.bitOffset + f.bitSize), 0);
  return (layout.reportId === 0 ? 0 : 1) + Math.ceil(bits / 8);
}

/** 一遍扫出所有 GET_DESCRIPTOR(0x22) 的数据阶段，按 wIndex 低字节（接口号）分组。 */
function collectReportDescriptors(context: DecodeContext): Map<number, CapturedDescriptor> {
  const found = new Map<number, CapturedDescriptor>();
  for (const exchange of context.exchanges) {
    const setup = exchange.setup;
    if (exchange.type !== "control" || setup === undefined || exchange.data.length === 0) continue;
    if (setup.bmRequestType !== 0x81 || setup.bRequest !== 0x06 || setup.wValue >> 8 !== 0x22) continue;
    const parsed = parseHidReportDescriptorDetailed(exchange.data);
    const fieldCount = parsed.layouts.reduce((total, layout) => total + layout.fields.length, 0);
    const interfaceNumber = setup.wIndex & 0xff;
    // 同一接口可能被重复 GET（枚举后重取）：保留信息量最多的那次，短读不覆盖完整描述符。
    if ((found.get(interfaceNumber)?.fieldCount ?? -1) > fieldCount) continue;
    found.set(interfaceNumber, { interfaceNumber, fieldCount, ...parsed });
  }
  return found;
}

/** 报告 → 布局：长度也对得上的优先；0 个或多于 1 个候选时不做猜测。 */
function pickLayout(env: HidEnv, reportId: number, byteLength: number): LayoutPick | undefined {
  const all = [...env.descriptors.values()];
  const exact = all.filter((entry) => {
    const layout = findLayout(entry, reportId);
    return layout !== undefined && expectedReportBytes(layout, entry.inputBits) === byteLength;
  });
  const pool = exact.length > 0 ? exact : all.filter((entry) => findLayout(entry, reportId) !== undefined);
  const captured = pool.length === 1 ? pool[0] : undefined;
  const layout = captured === undefined ? undefined : findLayout(captured, reportId);
  return captured !== undefined && layout !== undefined ? { captured, layout } : undefined;
}

/** 逐字段切片：constant 标 padding，variable 取值（signed 转补码），数组取前 N 个非零值。 */
function renderFields(pick: LayoutPick, data: Uint8Array, reportId: number): { lines: string[]; hits: string[]; anomalies: string[] } {
  const body = reportId === 0 ? data : data.subarray(1);
  const lines: string[] = [];
  const hits: string[] = [];
  const anomalies: string[] = [];
  let truncated = false;
  for (const field of pick.layout.fields) {
    const range = `bit ${field.bitOffset}..${field.bitOffset + Math.max(field.bitSize, 1) - 1}`;
    if (field.flags.constant) {
      lines.push(`${range}  padding (${field.bitSize} bits)`);
      continue;
    }
    if (field.flags.variable) {
      const raw = readBits(body, field.bitOffset, field.bitSize);
      if (raw === undefined) truncated = true;
      else {
        const value = field.signed ? signExtendBits(raw, field.bitSize) : raw;
        lines.push(`${range}  ${usageName(field.usagePage, field.usages[0])} = ${value}${field.flags.relative ? " (relative)" : ""}`);
        if (value !== 0) hits.push(`${fieldName(field)}=${value}`);
      }
      continue;
    }
    // 数组元素位宽来自描述符侧表；取不到时退化成整段一个值，不猜元素边界。
    const step = pick.captured.arrayElements.find((item) => item.reportId === reportId && item.bitOffset === field.bitOffset)?.elementBits ?? field.bitSize;
    const values: number[] = [];
    for (let index = 0; index < Math.floor(field.bitSize / Math.max(step, 1)); index += 1) {
      const raw = readBits(body, field.bitOffset + index * step, step);
      if (raw === undefined) { truncated = true; break; }
      const value = field.signed ? signExtendBits(raw, step) : raw;
      if (value !== 0 && values.length < MAX_VALUES) values.push(value);
    }
    const shown = values.length === 0 ? "[]" : `[${values.map((value) => hex(value)).join(" ")}]`;
    lines.push(`${range}  ${fieldName(field)} = ${shown}`);
    if (values.length > 0) hits.push(`${fieldName(field)}=${shown}`);
  }
  if (truncated) anomalies.push("报告在字段中途截断，之后的字段未解出");
  return { lines, hits, anomalies };
}

/** 中断 IN 的 HID 输入报告：布局解码 → boot 兜底 → 只给 ReportID 与长度并提示重抓。 */
function decodeInputReport(exchange: UsbExchange, env: HidEnv): DecodedMessage {
  const data = exchange.data;
  const reportId = env.usesReportIds ? byteAt(data, 0) ?? 0 : 0;
  const pick = env.descriptors.size === 0 ? undefined : pickLayout(env, reportId, data.length);
  const single = env.descriptors.size === 1 ? [...env.descriptors.values()][0] : undefined;
  const iface = env.hidInterfaces.find((item) => item.number === (pick?.captured.interfaceNumber ?? single?.interfaceNumber));
  // 只在没有描述符、或接口协议为 boot 时启用 boot 固定格式，避免和描述符布局冲突。
  const bootAllowed = env.descriptors.size === 0 || (iface !== undefined && (iface.protocolCode === 0 || iface.protocolCode === 2));
  const boot = bootAllowed ? decodeBootReport(data) : undefined;
  const idTag = reportId === 0 ? "" : `ID ${hex(reportId)} `;

  if (pick !== undefined && boot === undefined) {
    const rendered = renderFields(pick, data, reportId);
    const expected = expectedReportBytes(pick.layout, pick.captured.inputBits);
    const anomalies = [...rendered.anomalies];
    if (expected !== data.length) {
      // 长度不符本身是调试证据：仍然解出能对齐的字段。
      anomalies.push(`报告长度 ${data.length} 字节与布局 ${expected} 字节不符（${data.length > expected ? "多" : "少"} ${Math.abs(data.length - expected)} 字节），已按能对齐的字段解码`);
    }
    const detail = [
      `接口 ${pick.captured.interfaceNumber}，reportId ${hex(reportId)}（正文从第 ${reportId === 0 ? 0 : 1} 字节开始）`,
      `报告 ${data.length} 字节，布局 ${pick.layout.fields.length} 个字段（输入流 ${expected} 字节）`,
      `报告内容: ${toHex(data, 32)}`,
      ...rendered.lines.slice(0, MAX_FIELD_LINES),
    ];
    if (rendered.lines.length > MAX_FIELD_LINES) detail.push(`…共 ${rendered.lines.length} 行字段，仅显示前 ${MAX_FIELD_LINES} 行`);
    const hits = rendered.hits.length > 0 ? `: ${rendered.hits.join(" ")}` : ": 全部字段为 0";
    return msg(exchange, "Input report", `${idTag}len=${data.length}${hits}`, detail, anomalies, data);
  }

  // 描述符缺失、没有匹配布局或接口处于 boot 协议：只给 ReportID 与长度（boot 固定长度例外）。
  const hint =
    env.descriptors.size === 0
      ? MISSING_HINT
      : env.descriptors.size > 1
        ? `抓包里有 ${env.descriptors.size} 个接口的报告描述符，无法确定该中断端点归属哪个接口，未按字段解码`
        : `报告描述符里没有 reportId ${hex(reportId)} 且长度 ${data.length} 字节匹配的布局，无法按字段解码`;
  const anomalies = [env.descriptors.size === 0 ? "报告描述符不在抓包中，无法按字段解码" : "没有匹配的报告布局，无法按字段解码"];
  if (boot !== undefined) return msg(exchange, "Input report", `${boot.label} len=${data.length}: ${boot.summary}`, [hint, ...boot.lines], anomalies, data);
  return msg(exchange, "Input report", `${idTag}len=${data.length}`, [hint, `报告内容: ${toHex(data, 32)}`], anomalies, data);
}

// boot 键盘字节 0 的 modifier 位图（HID 1.11 附录 B.1）。
const BOOT_MODIFIER_BITS: Record<number, string> = { 0: "Left Ctrl", 1: "Left Shift", 2: "Left Alt", 3: "Left GUI", 4: "Right Ctrl", 5: "Right Shift", 6: "Right Alt", 7: "Right GUI" };

/** 只在 boot 固定长度上启用：8 字节键盘（modifier + 保留 + 6 按键码）、3/4 字节鼠标（按钮 + X/Y + 滚轮）。 */
function decodeBootReport(data: Uint8Array): { label: string; summary: string; lines: string[] } | undefined {
  if (data.length === 8) {
    const modifiers = byteAt(data, 0) ?? 0;
    const pressed = modifiers === 0 ? [] : setBitNames(modifiers, BOOT_MODIFIER_BITS);
    const reserved = byteAt(data, 1) ?? 0;
    const codes: number[] = [];
    for (let index = 2; index < 8; index += 1) {
      const code = byteAt(data, index);
      if (code !== undefined && code !== 0) codes.push(code);
    }
    const keys = codes.length > 0 ? `Keys[${codes.map((code) => hex(code)).join(" ")}]` : "";
    return {
      label: "Boot keyboard",
      summary: [pressed.join(" + "), keys].filter((part) => part !== "").join(" ") || "无按键",
      lines: [
        `modifier = ${hex(modifiers)}（${pressed.length > 0 ? pressed.join(" + ") : "无按键"}），按键码 page 0x07: ${codes.map((code) => usageName(KEYBOARD_PAGE, code)).join(" ") || "无"}`,
        ...(reserved === 0 ? [] : [`字节 1（规范保留）= ${hex(reserved)}（非 0：部分设备用它承载 Report ID 或厂商数据）`]),
      ],
    };
  }
  if (data.length === 3 || data.length === 4) {
    const buttons = byteAt(data, 0) ?? 0;
    const x = signedByte(byteAt(data, 1) ?? 0);
    const y = signedByte(byteAt(data, 2) ?? 0);
    const wheelRaw = byteAt(data, 3);
    const wheel = wheelRaw === undefined ? undefined : signedByte(wheelRaw);
    const names = buttons === 0 ? [] : setBitNames(buttons, { 0: "Left", 1: "Right", 2: "Middle" });
    const parts = [
      buttons === 0 ? "" : `Buttons ${hex(buttons)}${names.length > 0 ? `(${names.join("+")})` : ""}`,
      x !== 0 ? `X=${x}` : "",
      y !== 0 ? `Y=${y}` : "",
      wheel === undefined || wheel === 0 ? "" : `Wheel=${wheel}`,
    ].filter((part) => part !== "");
    return {
      label: "Boot mouse",
      summary: parts.join(" ") || "无按键/无位移",
      lines: [`按钮位图 = ${hex(buttons)}${names.length > 0 ? `（${names.join(" + ")}）` : "（无按键）"}；X=${x} Y=${y}${wheel === undefined ? "" : ` Wheel=${wheel}`}（8 位补码，相对位移）`],
    };
  }
  return undefined;
}

/** HID 类控制请求：SET_IDLE / SET_PROTOCOL / GET_REPORT / SET_REPORT。 */
function decodeControlExchange(exchange: UsbExchange, setup: UsbSetupPacket, env: HidEnv): DecodedMessage | undefined {
  // 只认 HID 类请求（bmRequestType 的 type 位 = class），避免把标准请求误当 SET_REPORT。
  if ((setup.bmRequestType & 0x60) !== 0x20) return undefined;
  const iface = setup.wIndex & 0xff;
  const reportId = setup.wValue & 0xff;
  if (setup.bRequest === HID_SET_IDLE) {
    const duration = setup.wValue >> 8;
    // SET_IDLE 是按键重复/丢报排查的关键证据：它决定设备多久重发一次相同报告。
    return msg(exchange, "Control request", `SET_IDLE iface=${iface} duration=${duration === 0 ? "infinite" : `${duration * 4}ms`} reportID=${hex(reportId)}`, [
      `duration = ${duration} × 4ms${duration === 0 ? "（无限：仅报告内容变化时上报）" : ""}，reportId = ${reportId === 0 ? "0x00（作用于该接口的所有报告）" : hex(reportId)}`,
    ]);
  }
  if (setup.bRequest === HID_SET_PROTOCOL) {
    const protocol = setup.wValue & 0xff;
    return msg(exchange, "Control request", `SET_PROTOCOL iface=${iface} protocol=${protocol === 0 ? "boot" : protocol === 1 ? "report" : hex(protocol)}`, [
      `protocol = ${protocol}（0 = boot，1 = report）：${protocol === 0 ? "之后的报告按 boot 固定格式解码" : "报告按报告描述符布局解码"}`,
    ]);
  }
  // GET_REPORT(0x01) / SET_REPORT(0x09)：wValue 高字节是报告类型、低字节是 reportId。
  if (setup.bRequest !== 0x01 && setup.bRequest !== 0x09) return undefined;
  const reportType = setup.wValue >> 8;
  const typeName = reportType === 1 ? "input" : reportType === 2 ? "output" : reportType === 3 ? "feature" : hex(reportType);
  const data = exchange.data;
  const detail = [`report type = ${reportType} (${typeName})，数据阶段 ${data.length} 字节 / wLength ${setup.wLength}`];
  if (data.length > 0) {
    detail.push(`数据: ${toHex(data, 32)}`);
    const pick = pickLayout(env, reportId, data.length);
    // 布局表不区分输入/输出/Feature，这里的字段切片只作参考。
    if (pick === undefined) detail.push("没有匹配的报告布局（描述符不在抓包中或 reportId 不符），无法按字段解码");
    else detail.push(...renderFields(pick, data, reportId).lines.slice(0, MAX_FIELD_LINES));
  }
  return msg(exchange, "Control request", `${setup.bRequest === 0x09 ? "SET_REPORT" : "GET_REPORT"} iface=${iface} type=${typeName} reportID=${hex(reportId)} len=${data.length > 0 ? data.length : setup.wLength}`, detail, [], data.length > 0 ? data : undefined);
}

/**
 * 解 HID 设备：入参是该设备（同一 busDev）的全部逻辑报文，按时间升序。
 * 只对确认是 HID 的设备产出消息：接口类 0x03，或抓包里出现过报告描述符。
 */
export function decodeHidMessages(context: DecodeContext): DecodedMessage[] {
  const descriptors = collectReportDescriptors(context);
  const hidInterfaces = context.device.interfaces.filter((iface) => iface.classCode === USB_CLASS.hid || iface.protocols.includes("hid"));
  // 确认不了 HID 就什么都不产出：宁可漏，也不要把别家协议的中断数据当 HID 解。
  if (descriptors.size === 0 && hidInterfaces.length === 0) return [];

  const env: HidEnv = { descriptors, hidInterfaces, usesReportIds: [...descriptors.values()].some((captured) => captured.layouts.some((layout) => layout.reportId !== 0)) };
  const messages: DecodedMessage[] = [];
  for (const exchange of context.exchanges) {
    const setup = exchange.setup;
    if (exchange.type === "control" && setup !== undefined) {
      const message = decodeControlExchange(exchange, setup, env);
      if (message !== undefined) messages.push(message);
    } else if (exchange.type === "interrupt" && exchange.data.length > 0) {
      // 中断 OUT 是 HID 输出报告（如键盘 LED）：布局不区分输入/输出位域，只给长度与内容。
      if (exchange.direction === "in") messages.push(decodeInputReport(exchange, env));
      else messages.push(msg(exchange, "Output report", `len=${exchange.data.length}`, [`报告内容: ${toHex(exchange.data, 32)}`], [], exchange.data));
    }
  }
  return messages;
}
