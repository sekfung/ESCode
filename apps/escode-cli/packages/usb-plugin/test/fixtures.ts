/**
 * 测试夹具：按容器与链路层的**公开格式**手工构造抓包字节。
 *
 * 这些夹具能验证解析器与格式定义一致，但**不能**证明我们对真实抓包的理解正确：
 * 真实 usbmon / USBPcap 文件的复核见 docs/specs/usb-agent-tools.md §11 的集成验收。
 */

export interface FixtureEvent {
  /** 微秒。 */
  ts: number;
  bus?: number;
  device?: number;
  /** 含方向位（0x80 = IN）。 */
  endpoint: number;
  type: 0 | 1 | 2 | 3;
  complete: boolean;
  setup?: { bmRequestType: number; bRequest: number; wValue: number; wIndex: number; wLength: number };
  data?: Uint8Array;
  status?: number;
  /** 覆盖 len_urb（提交侧声明长度），用于短传输用例。 */
  declaredLength?: number;
  /** URB id：同一事务的提交与完成要用同一个值才能走「按 id 配对」分支。 */
  urbId?: number;
}

function u32(value: number): number[] {
  return [value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff];
}

function u16(value: number): number[] {
  return [value & 0xff, (value >>> 8) & 0xff];
}

function u64(value: number): number[] {
  const bytes: number[] = [];
  let rest = value;
  for (let index = 0; index < 8; index += 1) {
    bytes.push(rest & 0xff);
    rest = Math.floor(rest / 256);
  }
  return bytes;
}

/** 64 字节的 `struct mon_bin_hdr`（LINKTYPE_USB_LINUX_MMAPPED）。 */
export function usbmonHeader(event: FixtureEvent, dataLength: number): Uint8Array {
  const header = new Uint8Array(64);
  const setupBytes = event.setup
    ? [
        event.setup.bmRequestType,
        event.setup.bRequest,
        ...u16(event.setup.wValue),
        ...u16(event.setup.wIndex),
        ...u16(event.setup.wLength),
      ]
    : undefined;
  const isControl = event.type === 2;
  header.set(u64(event.urbId ?? 0x1000 + event.ts), 0); // URB id：同一事务的提交与完成共用
  header[8] = event.complete ? 0x43 : 0x53; // 'C' / 'S'
  header[9] = event.type;
  header[10] = event.endpoint;
  header[11] = event.device ?? 5;
  header.set(u16(event.bus ?? 1), 12);
  header[14] = isControl && setupBytes ? 0x00 : 0x2d; // '-' = 非控制提交
  header[15] = event.data && event.data.length > 0 ? 0x00 : event.complete ? 0x3e : 0x3c;
  header.set(u64(Math.floor(event.ts / 1_000_000)), 16);
  header.set(u32(event.ts % 1_000_000), 24);
  header.set(u32(event.status ?? 0), 28);
  header.set(u32(event.declaredLength ?? dataLength), 32);
  header.set(u32(dataLength), 36);
  if (setupBytes) header.set(setupBytes, 40);
  return header;
}

function pad4(bytes: number[]): number[] {
  while (bytes.length % 4 !== 0) bytes.push(0);
  return bytes;
}

function enhancedPacketBlock(interfaceId: number, ts: number, payload: number[]): number[] {
  const body = [
    ...u32(interfaceId),
    ...u32(Math.floor(ts / 0x1_0000_0000)),
    ...u32(ts % 0x1_0000_0000),
    ...u32(payload.length),
    ...u32(payload.length),
    ...pad4(payload),
  ];
  const total = 8 + body.length + 4;
  return [0x06, 0x00, 0x00, 0x00, ...u32(total), ...body, ...u32(total)];
}

function sectionHeaderBlock(): number[] {
  const body = [0x4d, 0x3c, 0x2b, 0x1a, ...u16(1), ...u16(0), ...u64(0xffff_ffff)];
  const total = 8 + body.length + 4;
  return [0x0a, 0x0d, 0x0d, 0x0a, ...u32(total), ...body, ...u32(total)];
}

function interfaceDescriptionBlock(linkType: number, snaplen = 262_144): number[] {
  const body = [...u16(linkType), ...u16(0), ...u32(snaplen)];
  const total = 8 + body.length + 4;
  return [0x01, 0x00, 0x00, 0x00, ...u32(total), ...body, ...u32(total)];
}

/** usbmon（linktype 220）抓包文件；每条事件一个增强包块。 */
export function buildUsbmonPcapng(events: FixtureEvent[], linkType = 220): Uint8Array {
  const bytes = [...sectionHeaderBlock(), ...interfaceDescriptionBlock(linkType)];
  for (const event of events) {
    const payload = event.data ?? new Uint8Array(0);
    const header = usbmonHeader(event, payload.length);
    bytes.push(...enhancedPacketBlock(0, event.ts, [...header, ...payload]));
  }
  return new Uint8Array(bytes);
}

/** 经典 pcap 容器（linktype 由参数给出），用于验证老格式分支。 */export function buildClassicPcap(events: FixtureEvent[], linkType = 220): Uint8Array {
  const header = [
    0xd4,
    0xc3,
    0xb2,
    0xa1, // 小端 magic
    ...u16(2),
    ...u16(4),
    ...u32(0),
    ...u32(0),
    ...u32(262_144),
    ...u32(linkType),
  ];
  const bytes = [...header];
  for (const event of events) {
    const payload = event.data ?? new Uint8Array(0);
    const record = [...usbmonHeader(event, payload.length), ...payload];
    bytes.push(
      ...u32(Math.floor(event.ts / 1_000_000)),
      ...u32(event.ts % 1_000_000),
      ...u32(record.length),
      ...u32(record.length),
      ...record,
    );
  }
  return new Uint8Array(bytes);
}

export interface UsbpcapFixtureEvent {
  ts: number;
  bus?: number;
  device?: number;
  endpoint: number;
  transfer: 0 | 1 | 2 | 3;
  /** 控制传输的阶段：0 SETUP、1 DATA、2 STATUS、3 COMPLETE。 */
  stage?: number;
  /** true = 完成（info bit0 = 1，PDO→FDO），false = 提交（FDO→PDO）。 */
  complete: boolean;
  data?: Uint8Array;
  status?: number;
  irpId?: number;
}

/** USBPcap（linktype 249）抓包文件：27 字节伪头，控制传输再追加 stage 字节。 */
export function buildUsbpcapPcapng(events: UsbpcapFixtureEvent[], linkType = 249): Uint8Array {
  const bytes = [...sectionHeaderBlock(), ...interfaceDescriptionBlock(linkType)];
  for (const event of events) {
    const payload = event.data ?? new Uint8Array(0);
    const header = [
      ...u16(event.transfer === 2 ? 28 : 27),
      ...u64(event.irpId ?? 0x2000 + event.ts),
      ...u32(event.status ?? 0),
      ...u16(0),
      event.complete ? 0x01 : 0x00,
      ...u16(event.bus ?? 1),
      ...u16(event.device ?? 5),
      event.endpoint,
      event.transfer,
      ...u32(payload.length),
      ...(event.transfer === 2 ? [event.stage ?? 0] : []),
    ];
    bytes.push(...enhancedPacketBlock(0, event.ts, [...header, ...payload]));
  }
  return new Uint8Array(bytes);
}

/** 设备描述符（18 字节），vendorId/productId 用小端 u16 传入。 */
export function deviceDescriptor(vendorId: number, productId: number): Uint8Array {
  return new Uint8Array([
    18,
    0x01,
    0x00,
    0x02,
    0x00,
    0x00,
    0x00,
    64,
    ...u16(vendorId),
    ...u16(productId),
    0x00,
    0x01,
    0x00,
    0x00,
    0x00,
    1,
  ]);
}

/** 配置描述符 + 一个接口描述符（9 字节接口描述符，不含端点）。 */
export function configWithInterface(interfaceClass: number, subClass = 0, protocol = 0): Uint8Array {
  return new Uint8Array([
    9,
    0x02,
    18,
    0x00,
    1,
    1,
    0,
    0x80,
    50,
    9,
    0x04,
    0,
    0,
    0,
    interfaceClass,
    subClass,
    protocol,
    0,
  ]);
}

/** 控制传输的一对提交/完成事件（描述符读取）。 */
export function controlExchange(input: {
  ts: number;
  busDev?: { bus: number; device: number };
  wValue: number;
  wIndex?: number;
  data: Uint8Array;
  setup?: { bmRequestType?: number; bRequest?: number };
  device?: number;
}): FixtureEvent[] {
  const bus = input.busDev?.bus ?? 1;
  const device = input.busDev?.device ?? input.device ?? 5;
  const setup = {
    bmRequestType: input.setup?.bmRequestType ?? 0x80,
    bRequest: input.setup?.bRequest ?? 0x06,
    wValue: input.wValue,
    wIndex: input.wIndex ?? 0,
    wLength: input.data.length,
  };
  return [
    { ts: input.ts, bus, device, endpoint: 0x80, type: 2, complete: false, setup, declaredLength: 0 },
    {
      ts: input.ts + 200,
      bus,
      device,
      endpoint: 0x80,
      type: 2,
      complete: true,
      data: input.data,
      declaredLength: input.data.length,
    },
  ];
}

/** 批量传输的一对事件；IN 的数据在完成侧，OUT 的数据在提交侧（与真实抓包一致）。 */
export function bulkExchange(input: {
  ts: number;
  device?: number;
  endpoint: number;
  data: Uint8Array;
  status?: number;
}): FixtureEvent[] {
  const isIn = (input.endpoint & 0x80) !== 0;
  return [
    {
      ts: input.ts,
      device: input.device ?? 5,
      endpoint: input.endpoint,
      type: 3,
      complete: false,
      // OUT 的数据在提交事件里（主机发送时抓包），IN 的提交事件没有载荷。
      ...(isIn ? {} : { data: input.data }),
      declaredLength: input.data.length,
    },
    {
      ts: input.ts + 300,
      device: input.device ?? 5,
      endpoint: input.endpoint,
      type: 3,
      complete: true,
      ...(isIn ? { data: input.data } : {}),
      declaredLength: input.data.length,
      ...(input.status !== undefined ? { status: input.status } : {}),
    },
  ];
}

/** CCID `PC_to_RDR_XfrBlock` 报文：10 字节头（bBWI + wLevelParameter 为类型相关字段）+ APDU。 */
export function ccidXfrBlock(seq: number, apdu: number[]): Uint8Array {
  return new Uint8Array([0x6f, ...u32(apdu.length), 0, seq, 0, 0, 0, ...apdu]);
}

/** CCID `RDR_to_PC_DataBlock` 报文：10 字节头（bStatus + bError + bChainParameter）+ 响应与 SW1SW2。 */
export function ccidDataBlock(seq: number, body: number[]): Uint8Array {
  return new Uint8Array([0x80, ...u32(body.length), 0, seq, 0, 0, 0, ...body]);
}

export { u16, u32 };
