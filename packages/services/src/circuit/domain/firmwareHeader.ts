import {
  canonicalPortPin,
  normalizeFirmwareSignal,
  type FirmwarePinDeclaration,
} from "./firmwarePins.js";

/**
 * C 头文件引脚宏解析（规格 §7.1）：CubeMX 生成的 `main.h` 写法。
 *
 * 支持的构造：`#define <前缀>_PIN GPIO_PIN_<n>`（也接受 `LL_GPIO_PIN_<n>`）配同前缀的
 * `_GPIO_PORT` / `_GPIO_Port` / `_PORT` / `_Port` = `GPIO<口>`，如
 * `LED0_PIN = GPIO_PIN_5` + `LED0_GPIO_PORT = GPIOA` → `LED0` @ PA5。
 * 只写 `*_PIN` 没有端口宏、或端口不是 `GPIO<字母>` 的组合忽略（信息不全，不猜）。
 * 其它宏（寄存器、时钟、函数式宏）不参与比对。纯函数、零 IO；行号取 `_PIN` 宏所在行。
 */

const PIN_VALUE = /^(?:LL_)?GPIO_PIN_(\d{1,3})$/u;
const PORT_VALUE = /^GPIO([A-K])$/iu;

export function parseHeaderPins(params: {
  path: string;
  text: string;
}): FirmwarePinDeclaration[] {
  const pinByPrefix = new Map<string, { pin: string; line: number }>();
  const portByPrefix = new Map<string, string>();
  const lines = params.text.split("\n");
  for (const [index, raw] of lines.entries()) {
    const match = /^\s*#\s*define\s+([A-Za-z_]\w*)\s+(.+?)\s*$/u.exec(raw);
    if (!match) continue;
    const name = match[1]!;
    const value = match[2]!.replace(/\/\*.*$/u, "").trim();
    const pinSuffix = /^(.*?)_PIN$/iu.exec(name);
    if (pinSuffix && pinSuffix[1]!.length > 0) {
      const pin = PIN_VALUE.exec(value);
      if (pin) pinByPrefix.set(pinSuffix[1]!, { pin: pin[1]!, line: index + 1 });
      continue;
    }
    const portSuffix = /^(.*?)_(?:GPIO_)?PORT$/iu.exec(name);
    if (portSuffix && portSuffix[1]!.length > 0) {
      const port = PORT_VALUE.exec(value);
      if (port) portByPrefix.set(portSuffix[1]!, port[1]!);
    }
  }
  const declarations: FirmwarePinDeclaration[] = [];
  for (const [prefix, entry] of [...pinByPrefix].sort(([a], [b]) => a.localeCompare(b))) {
    const port = portByPrefix.get(prefix);
    if (port === undefined) continue;
    const signal = normalizeFirmwareSignal(prefix);
    const pin = canonicalPortPin(port, entry.pin);
    if (signal === undefined || pin === undefined) continue;
    declarations.push({ signal, pin, symbol: prefix, file: params.path, line: entry.line });
  }
  return declarations.sort((a, b) => a.line - b.line || a.signal.localeCompare(b.signal));
}
