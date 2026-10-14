/**
 * pcap / pcapng 容器读取（docs/specs/usb-agent-tools.md §7）。
 *
 * 只做容器层：把每个数据包连同它所属接口的链路类型与时间戳交出去，链路层解析在 usbmon.ts / usbpcap.ts。
 * 规则：**不猜格式**——容器或链路类型不认识就报 `unsupportedFormat`；文件被截断（抓包中途被杀）时
 * 读到坏块即停，返回已解析部分并置 `truncated`，绝不把截断当成功。
 */

export class CaptureFormatError extends Error {
  constructor(
    readonly code: "unsupportedFormat" | "decode",
    message: string,
  ) {
    super(message);
    this.name = "CaptureFormatError";
  }
}

export interface CaptureInterface {
  id: number;
  linkType: number;
  snaplen: number;
  name?: string;
  /** 时间戳单位：每秒多少个 tick（pcapng 的 if_tsresol）。 */
  ticksPerSecond: number;
}

export interface CaptureRecord {
  interfaceId: number;
  /** 该包所属接口的链路类型；调用方据此选择链路层解析器，不必自己查接口表。 */
  linkType: number;
  tsMicros: number;
  data: Uint8Array;
  originalLength: number;
}

export interface ReadCaptureResult {
  format: "pcap" | "pcapng";
  interfaces: CaptureInterface[];
  records: number;
  /** 文件在正常情况下提前结束（截断、坏块），只保证已产出的记录可用。 */
  truncated: boolean;
  warnings: string[];
}

const PCAP_MAGIC_MICROS = 0xa1b2c3d4;
const PCAP_MAGIC_NANOS = 0xa1b23c4d;
const BLOCK_SECTION_HEADER = 0x0a0d0d0a;
const BLOCK_INTERFACE_DESCRIPTION = 0x00000001;
const BLOCK_OBSOLETE_PACKET = 0x00000002;
const BLOCK_SIMPLE_PACKET = 0x00000003;
const BLOCK_ENHANCED_PACKET = 0x00000006;
const BYTE_ORDER_MAGIC = 0x1a2b3c4d;
/** 单个块的长度上限：512 MiB 的抓包也远小于它，超过即视为坏块而不是继续分配内存。 */
const MAX_BLOCK_BYTES = 512 * 1024 * 1024;

interface Reader {
  uint16(offset: number): number;
  uint32(offset: number): number;
  /** ts_high/ts_low 拼成的 64 位时间戳；避免 BigInt，按乘加还原。 */
  timestamp(highOffset: number): number;
}

function bigEndianReader(view: DataView): Reader {
  return {
    uint16: (offset) => view.getUint16(offset, false),
    uint32: (offset) => view.getUint32(offset, false),
    timestamp: (offset) =>
      view.getUint32(offset, false) * 0x1_0000_0000 + view.getUint32(offset + 4, false),
  };
}

function littleEndianReader(view: DataView): Reader {
  return {
    uint16: (offset) => view.getUint16(offset, true),
    uint32: (offset) => view.getUint32(offset, true),
    // pcapng 的时间戳是 timestamp_high 在前、timestamp_low 在后的两个 32 位字，
    // 整个 64 位值按段字节序存放：小端文件里高字先写但仍是同一个 64 位整数。
    timestamp: (offset) =>
      view.getUint32(offset, true) * 0x1_0000_0000 + view.getUint32(offset + 4, true),
  };
}

/** 解析 pcapng 选项区，只取我们关心的 `if_name`(2) 与 `if_tsresol`(9)。 */
function readInterfaceOptions(
  data: Uint8Array,
  view: DataView,
  reader: Reader,
  start: number,
  end: number,
): { name?: string; ticksPerSecond: number } {
  let offset = start;
  let name: string | undefined;
  let ticksPerSecond = 1_000_000;
  while (offset + 4 <= end) {
    const code = reader.uint16(offset);
    const length = reader.uint16(offset + 2);
    offset += 4;
    if (code === 0 || offset + length > end) break;
    if (code === 2) {
      // 接口名以 NUL 填充到 4 字节边界，去掉填充即可（不用控制字符正则）。
      name = new TextDecoder()
        .decode(data.subarray(offset, offset + length))
        .replaceAll("\u0000", "");
    } else if (code === 9 && length >= 1) {
      const raw = data[offset] ?? 0;
      ticksPerSecond = raw < 0x80 ? 10 ** raw : 2 ** (raw - 0x80);
    }
    offset += Math.ceil(length / 4) * 4;
  }
  return { name, ticksPerSecond: ticksPerSecond > 0 ? ticksPerSecond : 1_000_000 };
}

/** pcapng：块结构 + 每段自己的字节序，逐块扫。 */
function readPcapng(data: Uint8Array, onRecord: (record: CaptureRecord) => void): ReadCaptureResult {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const interfaces: CaptureInterface[] = [];
  const warnings: string[] = [];
  let reader: Reader | undefined;
  let records = 0;
  let truncated = false;
  let offset = 0;

  while (offset + 12 <= data.length) {
    // 段头块之前还不知道字节序，只能按大端读；段头块类型 0x0A0D0D0A 是回文，
    // 两种字节序下结果相同，因此这一步不会误判。之后的块必须用已确定的 reader 读。
    const blockType = reader ? reader.uint32(offset) : view.getUint32(offset);
    if (blockType === BLOCK_SECTION_HEADER) {
      if (view.getUint32(offset + 8) === BYTE_ORDER_MAGIC) {
        reader = bigEndianReader(view);
      } else if (view.getUint32(offset + 8, true) === BYTE_ORDER_MAGIC) {
        reader = littleEndianReader(view);
      } else {
        throw new CaptureFormatError("unsupportedFormat", "pcapng 段头缺少字节序标记");
      }
    } else if (!reader) {
      throw new CaptureFormatError("unsupportedFormat", "pcapng 文件缺少段头块");
    }
    const blockLength = reader.uint32(offset + 4);
    if (blockLength < 12 || blockLength % 4 !== 0 || blockLength > MAX_BLOCK_BYTES) {
      warnings.push(`偏移 ${offset} 处的块长度非法（${blockLength}），按截断文件处理`);
      truncated = true;
      break;
    }
    if (offset + blockLength > data.length) {
      warnings.push(`偏移 ${offset} 处的块跨越文件末尾，按截断文件处理`);
      truncated = true;
      break;
    }
    const trailer = reader.uint32(offset + blockLength - 4);
    if (trailer !== blockLength) {
      warnings.push(`偏移 ${offset} 处块首尾长度不一致，按截断文件处理`);
      truncated = true;
      break;
    }
    const body = offset + 8;
    const bodyBytes = blockLength - 12;
    if (blockType === BLOCK_INTERFACE_DESCRIPTION) {
      const linkType = reader.uint16(body);
      const snaplen = reader.uint32(body + 4);
      const options = readInterfaceOptions(data, view, reader, body + 8, offset + blockLength - 4);
      interfaces.push({
        id: interfaces.length,
        linkType,
        snaplen,
        ticksPerSecond: options.ticksPerSecond,
        ...(options.name ? { name: options.name } : {}),
      });
    } else if (blockType === BLOCK_ENHANCED_PACKET || blockType === BLOCK_OBSOLETE_PACKET) {
      // 两种块的数据区都在固定头部之后：EPB 是 interface_id(4) + ts(8) + cap(4) + orig(4)，
      // 已废弃的 Packet Block 是 interface_id(2) + drops(2) + 同样的 ts + cap + orig。
      const isEnhanced = blockType === BLOCK_ENHANCED_PACKET;
      const interfaceId = isEnhanced ? reader.uint32(body) : reader.uint16(body);
      const captured = reader.uint32(body + 12);
      const declared = reader.uint32(body + 16);
      const available = bodyBytes - 20;
      if (captured > available) {
        warnings.push(`偏移 ${offset} 处的包长度超出块范围，按截断文件处理`);
        truncated = true;
        break;
      }
      const link = interfaces[interfaceId];
      const ticks = link?.ticksPerSecond ?? 1_000_000;
      const rawTs = reader.timestamp(body + 4);
      onRecord({
        interfaceId,
        linkType: link?.linkType ?? -1,
        tsMicros: Math.round((rawTs * 1_000_000) / ticks),
        data: data.subarray(body + 20, body + 20 + captured),
        originalLength: declared,
      });
      records += 1;
    } else if (blockType === BLOCK_SIMPLE_PACKET) {
      // 简单包块没有时间戳与接口号，按接口 0、时间戳 0 处理（不编造时间）。
      const declared = reader.uint32(body);
      const captured = Math.min(declared, Math.max(0, bodyBytes - 4));
      onRecord({
        interfaceId: 0,
        linkType: interfaces[0]?.linkType ?? -1,
        tsMicros: 0,
        data: data.subarray(body + 4, body + 4 + captured),
        originalLength: declared,
      });
      records += 1;
    }
    offset += blockLength;
  }
  if (!truncated && offset !== data.length) {
    warnings.push(`文件尾部有 ${data.length - offset} 字节未识别内容，按截断文件处理`);
    truncated = true;
  }
  if (interfaces.length === 0) {
    throw new CaptureFormatError("unsupportedFormat", "pcapng 文件没有接口描述块");
  }
  return { format: "pcapng", interfaces, records, truncated, warnings };
}

/** 经典 pcap：固定 24 字节头 + 固定 16 字节记录头。魔数本身决定整份文件的字节序。 */
function readPcap(data: Uint8Array, onRecord: (record: CaptureRecord) => void): ReadCaptureResult {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const bigEndianMagic = view.getUint32(0, false);
  const littleEndianMagic = view.getUint32(0, true);
  const isBigEndian = bigEndianMagic === PCAP_MAGIC_MICROS || bigEndianMagic === PCAP_MAGIC_NANOS;
  const isLittleEndian = littleEndianMagic === PCAP_MAGIC_MICROS || littleEndianMagic === PCAP_MAGIC_NANOS;
  if (!isBigEndian && !isLittleEndian) {
    throw new CaptureFormatError("unsupportedFormat", "既不是 pcap 也不是 pcapng（魔数不匹配）");
  }
  const reader = isBigEndian ? bigEndianReader(view) : littleEndianReader(view);
  const nanos = (isBigEndian ? bigEndianMagic : littleEndianMagic) === PCAP_MAGIC_NANOS;
  const linkType = reader.uint32(20);
  const snaplen = reader.uint32(16);
  const warnings: string[] = [];
  let records = 0;
  let truncated = false;
  let offset = 24;
  while (offset + 16 <= data.length) {
    const tsSeconds = reader.uint32(offset);
    const tsFraction = reader.uint32(offset + 4);
    const captured = reader.uint32(offset + 8);
    const declared = reader.uint32(offset + 12);
    if (offset + 16 + captured > data.length) {
      warnings.push(`偏移 ${offset} 处的包跨越文件末尾，按截断文件处理`);
      truncated = true;
      break;
    }
    onRecord({
      interfaceId: 0,
      linkType,
      tsMicros: tsSeconds * 1_000_000 + Math.round(tsFraction / (nanos ? 1000 : 1)),
      data: data.subarray(offset + 16, offset + 16 + captured),
      originalLength: declared,
    });
    records += 1;
    offset += 16 + captured;
  }
  if (!truncated && offset !== data.length) {
    warnings.push(`文件尾部有 ${data.length - offset} 字节未识别内容，按截断文件处理`);
    truncated = true;
  }
  return {
    format: "pcap",
    interfaces: [{ id: 0, linkType, snaplen, ticksPerSecond: 1_000_000 }],
    records,
    truncated,
    warnings,
  };
}

/** 按魔数选择容器；未知容器抛 `unsupportedFormat`（不猜）。 */
export function readCaptureData(
  data: Uint8Array,
  onRecord: (record: CaptureRecord) => void,
): ReadCaptureResult {
  if (data.length < 12) {
    throw new CaptureFormatError("unsupportedFormat", "文件过短，不是抓包文件");
  }
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  return view.getUint32(0) === BLOCK_SECTION_HEADER
    ? readPcapng(data, onRecord)
    : readPcap(data, onRecord);
}
