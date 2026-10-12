/**
 * 固件侧引脚声明的公共模型（规格 §7）：解析结果与原理图 IR 比对都用它。
 *
 * 规范化是比对成立的前提：固件与原理图对同一引脚的写法不同（`<&gpiob 7>` / `GPIO_PIN_7` + `GPIOB` /
 * `PB7`），这里统一成规范引脚名；信号名统一成「大写 + 非字母数字转下划线」，两边同规则再比较。
 * 纯函数、零 IO。
 */

/** 一条固件引脚声明；`file` 是 workspace 相对路径，`line` 是 1 起始行号。 */
export interface FirmwarePinDeclaration {
  /** 固件侧信号名（节点标签 / 宏前缀 / 外设功能），已规范化。 */
  signal: string;
  /** 规范化后的 MCU 引脚名：`PA9` / `P0.26` / `GPIO12`；识别不出引脚时为 undefined。 */
  pin?: string;
  /**
   * 同一物理引脚的其它规范写法。只在「控制器 + 脚号」本身不自带端口语义时给出：
   * devicetree 的 `&gpio1 13` 既可能是 Nordic 的 `P1.13`，也可能是 ESP 的 `GPIO13`，
   * 两种写法都列出来，由比对时按 IR 里真实存在的引脚名命中（见 firmwareCompare）。
   */
  pinAliases?: string[];
  /** 固件里写下的原始符号名（节点名、宏前缀），报告里原样展示。 */
  symbol: string;
  file: string;
  line: number;
}

/** 信号名规范化：大写，非字母数字转 `_`，去掉首尾与重复的下划线；空值返回 undefined。 */
export function normalizeFirmwareSignal(name: string): string | undefined {
  const value = name
    .trim()
    .toUpperCase()
    .replace(/[^A-Z0-9]+/gu, "_")
    .replace(/^_+|_+$/gu, "");
  return value.length > 0 ? value : undefined;
}

/**
 * 原理图 IR 引脚名 → 规范引脚名：
 * - `PA9` / `PB12`（STM32 原理图符号）；
 * - `P0.26` / `P1.05`（Nordic）；
 * - `GPIO12` / `IO12`（ESP 系列与通用写法）。
 * 其它名字（`NRST`、`VDD`、`BOOT0`…）不是可分配的 GPIO，返回 undefined。
 */
export function canonicalMcuPinName(name: string | undefined): string | undefined {
  if (name === undefined) return undefined;
  const value = name.trim().toUpperCase();
  const letterPort = /^P([A-K])(\d{1,2})$/u.exec(value);
  if (letterPort) return `P${letterPort[1]}${Number(letterPort[2])}`;
  const numericPort = /^P(\d{1,2})\.(\d{1,2})$/u.exec(value);
  if (numericPort) return `P${Number(numericPort[1])}.${Number(numericPort[2])}`;
  const numericGpio = /^(?:GPIO|IO)(\d{1,2})$/u.exec(value);
  if (numericGpio) return `GPIO${Number(numericGpio[1])}`;
  return undefined;
}

/**
 * 固件侧「端口 + 脚号」→ 规范引脚名的候选列表（第一个是主写法）：
 * - 字母端口 `a`–`k`（`&gpiob 7`、`GPIOB` + `GPIO_PIN_7`）→ 只有 `PB7`；
 * - 数字端口（devicetree 的 `&gpio1 13`）没有端口语义：候选是 Nordic 的 `P1.13` 与
 *   ESP 系列的 `GPIO13`，比对时按 IR 里真实存在的引脚名命中（规格 §7.1）。
 */
export function canonicalPortPinCandidates(port: string, pinNumber: string): string[] {
  const pin = /^\d{1,3}$/u.exec(pinNumber.trim());
  if (!pin) return [];
  const digits = String(Number(pin[0]));
  const letter = /^([A-K])$/iu.exec(port.trim());
  if (letter) return [`P${letter[1]!.toUpperCase()}${digits}`];
  const numeric = /^\d{1,2}$/u.exec(port.trim());
  if (numeric) return [`GPIO${digits}`, `P${Number(numeric[0])}.${digits}`];
  return [];
}

export function canonicalPortPin(port: string, pinNumber: string): string | undefined {
  return canonicalPortPinCandidates(port, pinNumber)[0];
}

/**
 * 字符串比较用码点序（不用 localeCompare）：固件比对要跨 Windows / macOS / Linux 得到同样的
 * 「排序在后的声明」，locale 相关的排序会让冲突条目的定位在不同平台上漂移。
 */
export function compareFirmwareText(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** 按 `(file, line, signal, pin)` 排序并去重（同一 (signal, pin) 只保留首次出现）。 */
export function dedupeFirmwareDeclarations(
  declarations: FirmwarePinDeclaration[],
): FirmwarePinDeclaration[] {
  const sorted = [...declarations].sort(
    (a, b) =>
      compareFirmwareText(a.file, b.file) ||
      a.line - b.line ||
      compareFirmwareText(a.signal, b.signal) ||
      compareFirmwareText(a.pin ?? "", b.pin ?? ""),
  );
  const seen = new Set<string>();
  return sorted.filter((declaration) => {
    const key = `${declaration.signal}\u0000${declaration.pin ?? ""}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
