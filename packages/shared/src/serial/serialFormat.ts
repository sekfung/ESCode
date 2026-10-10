/** 串口收发编解码：UI 面板与 Host（Agent 串口工具）共用，保证两条路径对同一输入得到相同字节。 */
export type SerialSendMode = "text" | "hex";
export type SerialLineEnding = "none" | "cr" | "lf" | "crlf";
export type SerialDisplayEncoding = "utf-8" | "gbk";

export type SerialBytesResult =
  | { ok: true; bytes: Uint8Array }
  | { ok: false; error: "oddLength" | "invalidChar" };

const LINE_ENDINGS: Record<SerialLineEnding, string> = {
  none: "",
  cr: "\r",
  lf: "\n",
  crlf: "\r\n",
};

/** 解析 HEX 输入：允许空白/逗号分隔、大小写混用、每字节可带 0x 前缀；连续书写按两位一组。 */
export function parseSerialHexInput(input: string): SerialBytesResult {
  const tokens = input
    .split(/[\s,]+/)
    .filter(Boolean)
    .map((token) => token.replace(/^0x/i, ""));
  const digits = tokens.join("");
  if (/[^0-9a-f]/i.test(digits)) return { ok: false, error: "invalidChar" };
  if (tokens.some((token) => token.length % 2 !== 0)) return { ok: false, error: "oddLength" };
  const bytes = new Uint8Array(digits.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(digits.slice(index * 2, index * 2 + 2), 16);
  }
  return { ok: true, bytes };
}

export function formatSerialHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" ");
}

/** 文本模式按 UTF-8 编码并追加行尾；HEX 模式按字节原样发送，不追加行尾。 */
export function buildSerialSendPayload(options: {
  input: string;
  mode: SerialSendMode;
  lineEnding: SerialLineEnding;
}): SerialBytesResult {
  if (options.mode === "hex") return parseSerialHexInput(options.input);
  return {
    ok: true,
    bytes: new TextEncoder().encode(options.input + LINE_ENDINGS[options.lineEnding]),
  };
}

export interface SerialStreamDecoder {
  decode(bytes: Uint8Array): string;
}

/** 流式解码：多字节字符跨 chunk 时保留尾部残字节，等下一段到达后再输出。 */
export function createSerialStreamDecoder(encoding: SerialDisplayEncoding): SerialStreamDecoder {
  const decoder = new TextDecoder(encoding, { fatal: false });
  return {
    decode: (bytes) => decoder.decode(bytes, { stream: true }),
  };
}

const PREVIEW_ESCAPES: Record<number, string> = {
  0x09: "\\t",
  0x0a: "\\n",
  0x0d: "\\r",
  0x5c: "\\\\",
};

/**
 * 审批卡片用的可读预览：可打印 ASCII 原样保留，\r \n \t \\ 转义，其余字节写成 \xNN。只用于展示。
 */
export function escapeSerialPreview(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) {
    const escaped = PREVIEW_ESCAPES[byte];
    if (escaped) out += escaped;
    else if (byte >= 0x20 && byte < 0x7f) out += String.fromCharCode(byte);
    else out += `\\x${byte.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return out;
}
