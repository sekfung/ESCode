/**
 * 从抓包内容里还原设备与接口（docs/specs/usb-agent-tools.md §7「USB 基础层」）。
 *
 * 描述符只在设备枚举时出现在总线上：抓包若从设备已插好之后才开始，就看不到 GET_DESCRIPTOR。
 * 这时只能给出地址（`descriptorSource: "address"`）并把「让用户在抓包期间重新插拔」的建议交给上层，
 * 不能凭 VID:PID 之外的信息猜设备类别。
 */

import { readUint16LE } from "../format.js";
import {
  type DeviceInterface,
  type DeviceSummary,
  type UsbExchange,
  protocolsForInterfaceClass,
} from "../types.js";

const DESCRIPTOR_DEVICE = 0x01;
const DESCRIPTOR_CONFIGURATION = 0x02;
const DESCRIPTOR_STRING = 0x03;
const DESCRIPTOR_HID_REPORT = 0x22;
const DESCRIPTOR_INTERFACE = 0x04;

export interface ClassifyResult {
  devices: Map<string, DeviceSummary>;
  notes: string[];
}

function emptyDevice(busDev: string, bus: number, device: number): DeviceSummary {
  return { busDev, bus, device, interfaces: [], descriptorSource: "address" };
}

/** UTF-16LE 字符串描述符：前 2 字节是 bLength/bDescriptorType。 */
function parseStringDescriptor(data: Uint8Array): string | undefined {
  if (data.length < 4) return undefined;
  const length = Math.min(data[0] ?? 0, data.length);
  if (length < 4) return undefined;
  const raw = new TextDecoder("utf-16le").decode(data.subarray(2, length));
  // 控制字符会让结果难以阅读，也会污染上下文；按码点过滤（不用控制字符正则，避免 lint 与可读性双重问题）。
  const printable = Array.from(raw).filter((character) => {
    const code = character.codePointAt(0) ?? 0;
    return code > 0x1f && code !== 0x7f;
  });
  return printable.join("").trim();
}

/** 配置描述符是接口/端点描述符的容器：按 bLength 顺序走到下一个非接口项为止。 */
function parseInterfaces(config: Uint8Array, into: Map<number, DeviceInterface>): void {
  let offset = 0;
  while (offset + 2 <= config.length) {
    const length = config[offset] ?? 0;
    const type = config[offset + 1] ?? 0;
    if (length < 2 || offset + length > config.length) break;
    if (type === DESCRIPTOR_INTERFACE && length >= 9) {
      const number = config[offset + 2] ?? 0;
      const classCode = config[offset + 5] ?? 0;
      const subClassCode = config[offset + 6] ?? 0;
      const protocolCode = config[offset + 7] ?? 0;
      into.set(number, {
        number,
        classCode,
        subClassCode,
        protocolCode,
        protocols: protocolsForInterfaceClass(classCode),
      });
    }
    offset += length;
  }
}

/**
 * 扫描控制传输，得到每个设备（`busDev`）的描述符信息。
 * `addresses` 是所有出现过的设备地址——描述符缺失时它们仍要出现在结果里。
 */
export function classifyDevices(
  exchanges: UsbExchange[],
  addresses: Iterable<{ busDev: string; bus: number; device: number }>,
): ClassifyResult {
  const devices = new Map<string, DeviceSummary>();
  const notes: string[] = [];
  for (const address of addresses) {
    devices.set(address.busDev, emptyDevice(address.busDev, address.bus, address.device));
  }
  /** 设备描述符里的字符串索引要等字符串描述符出现后才能解析，先暂存。 */
  const stringIndexes = new Map<string, { manufacturer?: number; product?: number; serial?: number }>();
  const strings = new Map<string, string>();
  const interfaces = new Map<string, Map<number, DeviceInterface>>();
  const sawDescriptor = new Set<string>();

  for (const exchange of exchanges) {
    const setup = exchange.setup;
    if (!setup || exchange.data.length === 0) continue;
    const requestType = setup.bmRequestType;
    const isDescriptorRead = (requestType & 0x80) !== 0 && setup.bRequest === 0x06;
    if (!isDescriptorRead) continue;
    const device = devices.get(exchange.busDev);
    if (!device) continue;
    const descriptorType = (setup.wValue >> 8) & 0xff;
    const index = setup.wValue & 0xff;
    if (descriptorType === DESCRIPTOR_DEVICE && exchange.data.length >= 18) {
      const data = exchange.data;
      device.videoId = readUint16LE(data, 8);
      device.productId = readUint16LE(data, 10);
      stringIndexes.set(exchange.busDev, {
        manufacturer: data[14] ?? 0,
        product: data[15] ?? 0,
        serial: data[16] ?? 0,
      });
      sawDescriptor.add(exchange.busDev);
    } else if (descriptorType === DESCRIPTOR_CONFIGURATION) {
      const target = interfaces.get(exchange.busDev) ?? new Map<number, DeviceInterface>();
      parseInterfaces(exchange.data, target);
      interfaces.set(exchange.busDev, target);
      sawDescriptor.add(exchange.busDev);
    } else if (descriptorType === DESCRIPTOR_STRING) {
      const text = parseStringDescriptor(exchange.data);
      const language = setup.wIndex;
      if (text !== undefined && language !== 0) strings.set(`${exchange.busDev}:${index}`, text);
    } else if (descriptorType === DESCRIPTOR_HID_REPORT) {
      // 报告描述符本身由 HID 解码器使用；这里只记录「该设备确实是 HID」。
      sawDescriptor.add(exchange.busDev);
    }
  }

  for (const [busDev, device] of devices) {
    const deviceInterfaces = [...(interfaces.get(busDev)?.values() ?? [])].sort(
      (left, right) => left.number - right.number,
    );
    device.interfaces = deviceInterfaces;
    const indexes = stringIndexes.get(busDev);
    if (indexes) {
      const manufacturer = indexes.manufacturer
        ? strings.get(`${busDev}:${indexes.manufacturer}`)
        : undefined;
      const product = indexes.product ? strings.get(`${busDev}:${indexes.product}`) : undefined;
      const serial = indexes.serial ? strings.get(`${busDev}:${indexes.serial}`) : undefined;
      if (manufacturer) device.manufacturer = manufacturer;
      if (product) device.product = product;
      if (serial) device.serialNumber = serial;
    }
    if (sawDescriptor.has(busDev)) device.descriptorSource = "capture";
  }

  const withoutDescriptors = [...devices.values()].filter(
    (device) => device.descriptorSource === "address",
  );
  if (withoutDescriptors.length > 0 && devices.size > 0) {
    notes.push(
      `${withoutDescriptors.map((device) => device.busDev).join(", ")} 在抓包里没有描述符：` +
        "抓包很可能在设备枚举之后才开始。需要设备类别（CCID/HID/MSC）时，请让用户在抓包期间重新插拔设备。",
    );
  }
  return { devices, notes };
}
