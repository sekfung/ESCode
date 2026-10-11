import {
  canonicalPortPin,
  normalizeFirmwareSignal,
  type FirmwarePinDeclaration,
} from "./firmwarePins.js";

/**
 * STM32CubeMX `.ioc` 解析（规格 §7.1）。
 *
 * 支持的构造：`P<口><脚>.Signal=<外设功能>`（如 `PA9.Signal=USART1_TX`）。
 * 通用 GPIO 值（`GPIO_Input` / `GPIO_Output` / `GPIO_Analog` …）在 CubeMX 里没有外设功能名，
 * 这时用同组的 `.GPIO_Label=` 当信号名（如 `PB6.GPIO_Label=LED0`）；没有标签就跳过。
 * 其它键（`Mode` / `PinState` / `Locked` / `Mcu.Pin…`）不参与比对。
 *
 * 纯函数、零 IO；行号取声明所在行（通用 GPIO 取 `.GPIO_Label=` 行）。
 */

export function parseIocPins(params: {
  path: string;
  text: string;
}): FirmwarePinDeclaration[] {
  const byPin = new Map<
    string,
    { symbol?: { value: string; line: number }; label?: { value: string; line: number } }
  >();
  const lines = params.text.split("\n");
  for (const [index, raw] of lines.entries()) {
    const match = /^(P[A-K]\d{1,2})\.([A-Za-z_]+)\s*=\s*(.*)$/u.exec(raw.trim());
    if (!match) continue;
    const pin = match[1]!;
    const key = match[2]!;
    const value = match[3]!.trim();
    if (value.length === 0) continue;
    const entry = byPin.get(pin) ?? {};
    if (key === "Signal") entry.symbol = { value, line: index + 1 };
    else if (key === "GPIO_Label") entry.label = { value, line: index + 1 };
    byPin.set(pin, entry);
  }
  const declarations: FirmwarePinDeclaration[] = [];
  for (const [pinName, entry] of [...byPin].sort(([a], [b]) => a.localeCompare(b))) {
    const generic = entry.symbol !== undefined && /^GPIO_/iu.test(entry.symbol.value);
    const source = generic ? entry.label : (entry.symbol ?? entry.label);
    if (source === undefined) continue;
    const signal = normalizeFirmwareSignal(source.value);
    const pin = canonicalPortPin(pinName.slice(1, 2), pinName.slice(2));
    if (signal === undefined || pin === undefined) continue;
    declarations.push({ signal, pin, symbol: pinName, file: params.path, line: source.line });
  }
  return declarations.sort((a, b) => a.line - b.line || a.signal.localeCompare(b.signal));
}
