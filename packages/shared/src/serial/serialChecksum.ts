import { z } from "zod";

/**
 * 串口校验和（docs/specs/serial-port-debugger-phase3.md 第 5 节）。参数均为常见标准：
 * - CRC-8：poly 0x07、init 0x00、不反射、xorout 0x00（check 0xF4）
 * - CRC-16/MODBUS：poly 0x8005 反射、init 0xFFFF（check 0x4B37），惯例低字节在前
 * - CRC-16/CCITT-FALSE：poly 0x1021、init 0xFFFF、不反射（check 0x29B1）
 * - CRC-32：IEEE 802.3 反射、init/xorout 0xFFFFFFFF（check 0xCBF43926）
 * - XOR（BCC）、SUM8（字节和取低 8 位）、LRC（字节和的二进制补码）
 */
export const SERIAL_CHECKSUM_ALGORITHMS = [
  "xor",
  "sum8",
  "crc8",
  "crc16-modbus",
  "crc16-ccitt-false",
  "crc32",
  "lrc",
] as const;

export type SerialChecksumAlgorithm = (typeof SERIAL_CHECKSUM_ALGORITHMS)[number];

export const serialChecksumConfigSchema = z
  .object({
    algorithm: z.enum(SERIAL_CHECKSUM_ALGORITHMS),
    /** 跳过开头 N 字节不参与计算（如帧头 AA 55）。 */
    skip: z.number().int().min(0).max(1024).optional(),
    /** 多字节结果的字节序；缺省按算法惯例（MODBUS 小端，其余大端）。 */
    endian: z.enum(["big", "little"]).optional(),
  })
  .strict();

export type SerialChecksumConfig = z.infer<typeof serialChecksumConfigSchema>;

const WIDTH: Record<SerialChecksumAlgorithm, number> = {
  xor: 1,
  sum8: 1,
  crc8: 1,
  "crc16-modbus": 2,
  "crc16-ccitt-false": 2,
  crc32: 4,
  lrc: 1,
};

const CONVENTIONAL_ENDIAN: Record<SerialChecksumAlgorithm, "big" | "little"> = {
  xor: "big",
  sum8: "big",
  crc8: "big",
  "crc16-modbus": "little",
  "crc16-ccitt-false": "big",
  crc32: "big",
  lrc: "big",
};

export function serialChecksumWidth(algorithm: SerialChecksumAlgorithm): number {
  return WIDTH[algorithm];
}

function compute(algorithm: SerialChecksumAlgorithm, data: Uint8Array): number {
  switch (algorithm) {
    case "xor":
      return data.reduce((acc, byte) => acc ^ byte, 0);
    case "sum8":
      return data.reduce((acc, byte) => acc + byte, 0) & 0xff;
    case "lrc":
      return -data.reduce((acc, byte) => acc + byte, 0) & 0xff;
    case "crc8": {
      let crc = 0;
      for (const byte of data) {
        crc ^= byte;
        for (let bit = 0; bit < 8; bit += 1) {
          crc = crc & 0x80 ? ((crc << 1) ^ 0x07) & 0xff : (crc << 1) & 0xff;
        }
      }
      return crc;
    }
    case "crc16-modbus": {
      let crc = 0xffff;
      for (const byte of data) {
        crc ^= byte;
        for (let bit = 0; bit < 8; bit += 1) {
          crc = crc & 1 ? (crc >>> 1) ^ 0xa001 : crc >>> 1;
        }
      }
      return crc;
    }
    case "crc16-ccitt-false": {
      let crc = 0xffff;
      for (const byte of data) {
        crc ^= byte << 8;
        for (let bit = 0; bit < 8; bit += 1) {
          crc = crc & 0x8000 ? ((crc << 1) ^ 0x1021) & 0xffff : (crc << 1) & 0xffff;
        }
      }
      return crc;
    }
    case "crc32": {
      let crc = 0xffffffff;
      for (const byte of data) {
        crc ^= byte;
        for (let bit = 0; bit < 8; bit += 1) {
          crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
        }
      }
      return (crc ^ 0xffffffff) >>> 0;
    }
  }
}

/** 计算校验和字节（按配置的字节序）；覆盖范围为跳过开头 skip 字节后的全部数据。 */
export function computeSerialChecksum(data: Uint8Array, config: SerialChecksumConfig): Uint8Array {
  const width = WIDTH[config.algorithm];
  const value = compute(config.algorithm, data.subarray(config.skip ?? 0));
  const bigEndian = Array.from(
    { length: width },
    (_, index) => (value >>> (8 * (width - 1 - index))) & 0xff,
  );
  const endian = config.endian ?? CONVENTIONAL_ENDIAN[config.algorithm];
  return Uint8Array.from(endian === "big" ? bigEndian : bigEndian.reverse());
}

const toHex = (bytes: Uint8Array) =>
  Array.from(bytes, (byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" ");

/**
 * 帧校验：以末尾 k 字节为校验和、其前面的字节（遵守 skip）为覆盖范围。
 * 帧短于校验和宽度时视为失败，expected 为空。
 */
export function verifySerialChecksum(
  frame: Uint8Array,
  config: SerialChecksumConfig,
): { ok: boolean; expected: string; actual: string } {
  const width = WIDTH[config.algorithm];
  if (frame.byteLength < width + (config.skip ?? 0)) {
    return {
      ok: false,
      expected: "",
      actual: toHex(frame.subarray(Math.max(0, frame.byteLength - width))),
    };
  }
  const body = frame.subarray(0, frame.byteLength - width);
  const expected = toHex(computeSerialChecksum(body, config));
  const actual = toHex(frame.subarray(frame.byteLength - width));
  return { ok: expected === actual, expected, actual };
}
