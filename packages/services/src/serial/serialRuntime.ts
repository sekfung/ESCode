import type { BindingInterface } from "@serialport/bindings-cpp";
import { SerialError, type SerialConfig } from "./serial.js";

const VALID_DATA_BITS = new Set([5, 6, 7, 8]);
const VALID_PARITY = new Set(["none", "even", "odd", "mark", "space"]);
const VALID_STOP_BITS = new Set([1, 1.5, 2]);

export type SerialStreamModule = typeof import("@serialport/stream");
let streamModulePromise: Promise<SerialStreamModule> | null = null;

// @serialport/* 是 CJS 包；server 会把 services 内联进 ESM bundle，静态 import 会在启动时
// 触发 Dynamic require 崩溃。串口只在 Desktop Local Host 使用，因此全部延迟到首次打开时加载。
export function loadStreamModule(): Promise<SerialStreamModule> {
  streamModulePromise ??= import("@serialport/stream").catch((error: unknown) => {
    streamModulePromise = null;
    throw new SerialError(
      "nativeUnavailable",
      error instanceof Error ? error.message : String(error),
    );
  });
  return streamModulePromise;
}

async function loadNativeBinding(): Promise<BindingInterface> {
  const module = await import("@serialport/bindings-cpp");
  return module.autoDetect();
}

/**
 * E2E 专用接缝：设置 ZCODE_SERIAL_MOCK_PORTS（逗号分隔）时改用 binding-mock 的回环虚拟串口，
 * 不触碰真实硬件。binding-mock 只是开发依赖，安装包里不存在，生产环境即使误设也只会报 nativeUnavailable。
 */
async function loadMockBinding(paths: string[]): Promise<BindingInterface> {
  // binding-mock 的 exports 未声明 types，且已在打包配置中外置；用变量模块名按运行时依赖加载。
  const specifier = "@serialport/binding-mock";
  const { MockBinding } = (await import(specifier)) as {
    MockBinding: BindingInterface & {
      createPort(path: string, options?: { echo?: boolean; manufacturer?: string }): void;
    };
  };
  const existing = new Set((await MockBinding.list()).map((port) => port.path));
  for (const path of paths) {
    if (!existing.has(path)) MockBinding.createPort(path, { echo: true, manufacturer: "Mock" });
  }
  return {
    list: async () => (await MockBinding.list()).filter((port) => paths.includes(port.path)),
    open: (options) => MockBinding.open(options),
  };
}

export function loadDefaultSerialBinding(): Promise<BindingInterface> {
  const mockPorts = process.env.ZCODE_SERIAL_MOCK_PORTS?.split(",")
    .map((path) => path.trim())
    .filter(Boolean);
  return mockPorts?.length ? loadMockBinding(mockPorts) : loadNativeBinding();
}

export function validateConfig(path: string, config: SerialConfig): void {
  const problems: string[] = [];
  if (typeof path !== "string" || path.trim() === "") problems.push("path");
  if (!Number.isInteger(config.baudRate) || config.baudRate <= 0) problems.push("baudRate");
  if (!VALID_DATA_BITS.has(config.dataBits)) problems.push("dataBits");
  if (!VALID_PARITY.has(config.parity)) problems.push("parity");
  if (!VALID_STOP_BITS.has(config.stopBits)) problems.push("stopBits");
  if (problems.length > 0) {
    throw new SerialError("invalidConfig", `Invalid serial config: ${problems.join(", ")}`);
  }
}
