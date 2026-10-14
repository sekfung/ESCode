/**
 * 视图层的紧凑格式化（docs/specs/usb-agent-tools.md §8）。
 *
 * 抓包动辄数万条，任何字符串拼接都必须有上限：这里只提供定长/限量助手，
 * 协议解码器不得自行 `Array.from(...).join(" ")` 拼整包正文。
 */

/** 读取越界返回 undefined，调用方据此判断长度不足而不是拿到 undefined 参与运算。 */
export function byteAt(data: Uint8Array, offset: number): number | undefined {
  return offset >= 0 && offset < data.length ? data[offset] : undefined;
}

export function readUint16BE(data: Uint8Array, offset: number): number | undefined {
  const high = byteAt(data, offset);
  const low = byteAt(data, offset + 1);
  return high === undefined || low === undefined ? undefined : (high << 8) | low;
}

export function readUint16LE(data: Uint8Array, offset: number): number | undefined {
  const high = byteAt(data, offset + 1);
  const low = byteAt(data, offset);
  return high === undefined || low === undefined ? undefined : (high << 8) | low;
}

export function readUint32BE(data: Uint8Array, offset: number): number | undefined {
  const a = byteAt(data, offset);
  const b = byteAt(data, offset + 1);
  const c = byteAt(data, offset + 2);
  const d = byteAt(data, offset + 3);
  if (a === undefined || b === undefined || c === undefined || d === undefined) return undefined;
  return ((a << 24) | (b << 16) | (c << 8) | d) >>> 0;
}

export function readUint32LE(data: Uint8Array, offset: number): number | undefined {
  const a = byteAt(data, offset + 3);
  const b = byteAt(data, offset + 2);
  const c = byteAt(data, offset + 1);
  const d = byteAt(data, offset);
  if (a === undefined || b === undefined || c === undefined || d === undefined) return undefined;
  return ((a << 24) | (b << 16) | (c << 8) | d) >>> 0;
}

/** `1F 2B` 风格；超过 `limit` 字节时以 `…(共 N 字节)` 收尾。 */
export function toHex(data: Uint8Array, limit = data.length): string {
  const shown = data.subarray(0, Math.max(0, Math.min(limit, data.length)));
  const hex = Array.from(shown, (byte) => byte.toString(16).padStart(2, "0").toUpperCase()).join(" ");
  return shown.length < data.length ? `${hex} …(共 ${data.length} 字节)` : hex;
}

/** 只保留可打印 ASCII，其余以 `.` 占位，避免控制字符进入上下文。 */
export function toAscii(data: Uint8Array, limit = data.length): string {
  const shown = data.subarray(0, Math.max(0, Math.min(limit, data.length)));
  return Array.from(shown, (byte) =>
    byte >= 0x20 && byte <= 0x7e ? String.fromCharCode(byte) : ".",
  ).join("");
}

/** 取 `count` 字节无符号小端整数；越界返回 undefined。 */
export function readBytesLE(data: Uint8Array, offset: number, count: number): number | undefined {
  let value = 0;
  for (let index = count - 1; index >= 0; index -= 1) {
    const byte = byteAt(data, offset + index);
    if (byte === undefined) return undefined;
    value = value * 256 + byte;
  }
  return value;
}

/** 十六进制字节串（`0x1F` 风格），用于错误信息与常量比对。 */
export function hex(value: number, width = 2): string {
  return `0x${value.toString(16).padStart(width, "0").toUpperCase()}`;
}

/** 位域解码：`{ 6: "CommandStatus" }` + 值 `0x40` → `["CommandStatus(bit6)"]`。 */
export function setBitNames(value: number, table: Record<number, string>): string[] {
  return Object.entries(table)
    .filter(([bit]) => (value & (1 << Number(bit))) !== 0)
    .map(([bit, name]) => `${name}(bit${bit})`);
}
