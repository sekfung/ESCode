import { CircuitServiceError } from "./circuitErrors.js";
import { parseDevicetreePins } from "./firmwareDevicetree.js";
import { parseHeaderPins } from "./firmwareHeader.js";
import { parseIocPins } from "./firmwareIoc.js";
import type { FirmwarePinDeclaration } from "./firmwarePins.js";

/**
 * 固件来源分派（规格 §7、§7.1）：按扩展名决定解析器。
 * 只支持 devicetree（`.dts` / `.dtsi` / `.overlay`）、STM32CubeMX（`.ioc`）与 C 头文件（`.h`）；
 * Arduino / PlatformIO 这类散落在代码里的写法不经过比对器（§7 正文）。
 */
export type CircuitFirmwareFormat = "devicetree" | "ioc" | "header";

function detectFirmwareFormat(path: string): CircuitFirmwareFormat | undefined {
  const lower = path.toLowerCase();
  if (lower.endsWith(".dts") || lower.endsWith(".dtsi") || lower.endsWith(".overlay"))
    return "devicetree";
  if (lower.endsWith(".ioc")) return "ioc";
  if (lower.endsWith(".h")) return "header";
  return undefined;
}

/** 扩展名不支持时抛 `unsupported_format`；调用方可以在读文件之前先判格式。 */
export function assertFirmwareFormat(path: string): CircuitFirmwareFormat {
  const format = detectFirmwareFormat(path);
  if (format === undefined)
    throw new CircuitServiceError(
      "unsupported_format",
      `固件比对只支持 .dts / .dtsi / .overlay / .ioc / .h（收到 ${path}）`,
    );
  return format;
}

export function parseFirmwarePins(params: {
  path: string;
  text: string;
}): FirmwarePinDeclaration[] {
  switch (assertFirmwareFormat(params.path)) {
    case "devicetree":
      return parseDevicetreePins(params);
    case "ioc":
      return parseIocPins(params);
    case "header":
      return parseHeaderPins(params);
  }
}
