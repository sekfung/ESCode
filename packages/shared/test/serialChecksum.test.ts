import assert from "node:assert/strict";
import test from "node:test";
import {
  buildSerialSendPayload,
  computeSerialChecksum,
  formatSerialHex,
  SERIAL_CHECKSUM_ALGORITHMS,
  serialChecksumConfigSchema,
  verifySerialChecksum,
} from "../src/serial/index.js";

const CHECK_INPUT = new TextEncoder().encode("123456789");
const hex = (bytes: Uint8Array) => formatSerialHex(bytes);

// 公开的标准 check 值（reveng CRC catalogue；XOR/SUM8/LRC 按定义计算）
const CHECK_VALUES: Record<string, { value: string; defaultEndian: "big" | "little" }> = {
  xor: { value: "31", defaultEndian: "big" },
  sum8: { value: "DD", defaultEndian: "big" },
  crc8: { value: "F4", defaultEndian: "big" },
  "crc16-modbus": { value: "37 4B", defaultEndian: "little" },
  "crc16-ccitt-false": { value: "29 B1", defaultEndian: "big" },
  crc32: { value: "CB F4 39 26", defaultEndian: "big" },
  lrc: { value: "23", defaultEndian: "big" },
};

test("每种算法的标准 check 值与默认字节序", () => {
  assert.deepEqual([...SERIAL_CHECKSUM_ALGORITHMS].sort(), Object.keys(CHECK_VALUES).sort());
  for (const [algorithm, expected] of Object.entries(CHECK_VALUES)) {
    const result = computeSerialChecksum(CHECK_INPUT, { algorithm: algorithm as never });
    assert.equal(hex(result), expected.value, algorithm);
  }
});

test("多字节校验和可指定字节序", () => {
  assert.equal(
    hex(computeSerialChecksum(CHECK_INPUT, { algorithm: "crc16-modbus", endian: "big" })),
    "4B 37",
  );
  assert.equal(
    hex(computeSerialChecksum(CHECK_INPUT, { algorithm: "crc32", endian: "little" })),
    "26 39 F4 CB",
  );
});

test("跳过开头 N 字节不参与计算", () => {
  const framed = new Uint8Array([0xaa, 0x55, ...CHECK_INPUT]);
  assert.equal(hex(computeSerialChecksum(framed, { algorithm: "crc8", skip: 2 })), "F4");
});

test("发送时校验和附加在内容之后、行尾之前", () => {
  const payload = buildSerialSendPayload({
    input: "123456789",
    mode: "text",
    lineEnding: "crlf",
    checksum: { algorithm: "sum8" },
  });
  assert.ok(payload.ok);
  assert.equal(
    hex(payload.ok ? payload.bytes : new Uint8Array()),
    "31 32 33 34 35 36 37 38 39 DD 0D 0A",
  );
  const hexPayload = buildSerialSendPayload({
    input: "AA 55 01 02",
    mode: "hex",
    lineEnding: "none",
    checksum: { algorithm: "xor", skip: 2 },
  });
  assert.equal(hex(hexPayload.ok ? hexPayload.bytes : new Uint8Array()), "AA 55 01 02 03");
});

test("帧校验：末尾 k 字节为校验和，覆盖范围遵守跳过字节", () => {
  const good = new Uint8Array([0xaa, 0x55, ...CHECK_INPUT, 0xf4]);
  assert.deepEqual(verifySerialChecksum(good, { algorithm: "crc8", skip: 2 }), {
    ok: true,
    expected: "F4",
    actual: "F4",
  });
  const bad = new Uint8Array([...CHECK_INPUT, 0x37, 0x4c]);
  assert.deepEqual(verifySerialChecksum(bad, { algorithm: "crc16-modbus" }), {
    ok: false,
    expected: "37 4B",
    actual: "37 4C",
  });
  assert.deepEqual(verifySerialChecksum(new Uint8Array([0x01]), { algorithm: "crc16-modbus" }), {
    ok: false,
    expected: "",
    actual: "01",
  });
});

test("校验和配置 schema", () => {
  assert.ok(
    serialChecksumConfigSchema.safeParse({ algorithm: "crc32", skip: 1, endian: "little" }).success,
  );
  assert.ok(!serialChecksumConfigSchema.safeParse({ algorithm: "md5" }).success);
  assert.ok(!serialChecksumConfigSchema.safeParse({ algorithm: "xor", skip: -1 }).success);
});
