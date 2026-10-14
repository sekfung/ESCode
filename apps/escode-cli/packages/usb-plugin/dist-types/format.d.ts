/**
 * 视图层的紧凑格式化（docs/specs/usb-agent-tools.md §8）。
 *
 * 抓包动辄数万条，任何字符串拼接都必须有上限：这里只提供定长/限量助手，
 * 协议解码器不得自行 `Array.from(...).join(" ")` 拼整包正文。
 */
/** 读取越界返回 undefined，调用方据此判断长度不足而不是拿到 undefined 参与运算。 */
export declare function byteAt(data: Uint8Array, offset: number): number | undefined;
export declare function readUint16BE(data: Uint8Array, offset: number): number | undefined;
export declare function readUint16LE(data: Uint8Array, offset: number): number | undefined;
export declare function readUint32BE(data: Uint8Array, offset: number): number | undefined;
export declare function readUint32LE(data: Uint8Array, offset: number): number | undefined;
/** `1F 2B` 风格；超过 `limit` 字节时以 `…(共 N 字节)` 收尾。 */
export declare function toHex(data: Uint8Array, limit?: number): string;
/** 只保留可打印 ASCII，其余以 `.` 占位，避免控制字符进入上下文。 */
export declare function toAscii(data: Uint8Array, limit?: number): string;
/** 取 `count` 字节无符号小端整数；越界返回 undefined。 */
export declare function readBytesLE(data: Uint8Array, offset: number, count: number): number | undefined;
/** 十六进制字节串（`0x1F` 风格），用于错误信息与常量比对。 */
export declare function hex(value: number, width?: number): string;
//# sourceMappingURL=format.d.ts.map