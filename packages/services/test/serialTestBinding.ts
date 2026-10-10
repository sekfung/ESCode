import { MockBinding } from "@serialport/binding-mock";
import type { BindingInterface, BindingPortInterface } from "@serialport/bindings-cpp";
import type { SerialConfig } from "../src/serial/serial.js";

export const TEST_SERIAL_CONFIG: SerialConfig = {
  baudRate: 115200,
  dataBits: 8,
  parity: "none",
  stopBits: 1,
  rtscts: false,
  autoReconnect: true,
};

/**
 * MockBinding 不支持模拟拔出；这里包一层：拔出时让在途 read 失败（stream 据此判定 disconnected），
 * 并把该路径从 list() 中隐藏，重新插入时恢复。
 */
export function createTestBinding() {
  const unplugged = new Set<string>();
  const live = new Map<string, { port: BindingPortInterface; unplug: () => void }>();
  let listCalls = 0;
  const binding: BindingInterface = {
    async list() {
      listCalls += 1;
      return (await MockBinding.list()).filter((info) => !unplugged.has(info.path));
    },
    async open(options) {
      if (unplugged.has(options.path)) {
        throw new Error(
          `Port does not exist - please call MockBinding.createPort('${options.path}') first`,
        );
      }
      const port = await MockBinding.open(options);
      let fail: (error: Error) => void = () => {};
      const unplugSignal = new Promise<never>((_, reject) => {
        fail = reject;
      });
      unplugSignal.catch(() => {});
      const proxy = new Proxy(port, {
        get(target, key) {
          if (key === "read") {
            return (buffer: Buffer, offset: number, length: number) =>
              Promise.race([target.read(buffer, offset, length), unplugSignal]);
          }
          const value = Reflect.get(target, key) as unknown;
          return typeof value === "function" ? value.bind(target) : value;
        },
      });
      live.set(options.path, {
        port,
        unplug: () => fail(new Error("device disconnected")),
      });
      return proxy;
    },
  };
  return {
    binding,
    get listCalls() {
      return listCalls;
    },
    unplug(path: string) {
      unplugged.add(path);
      live.get(path)?.unplug();
    },
    replug(path: string) {
      unplugged.delete(path);
    },
    emit(path: string, data: string | Buffer) {
      const entry = live.get(path);
      if (!entry) throw new Error(`port not opened: ${path}`);
      (entry.port as unknown as { emitData(data: string | Buffer): void }).emitData(data);
    },
  };
}

export async function waitFor(
  predicate: () => boolean | Promise<boolean>,
  timeoutMs = 2000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(await predicate())) {
    if (Date.now() > deadline) throw new Error("waitFor timed out");
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}
