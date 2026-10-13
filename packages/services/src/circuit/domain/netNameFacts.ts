/**
 * 网络名与引脚名的确定性约定（纯函数、零 IO）。
 * IR v1 不携带引脚电气类型（`power_in` / `no_connect`），因此这些判定用命名约定做代理：
 * 固件比对用它跳过地网、电源网与解析器合成的网名。
 */

/**
 * 电源输入引脚名：KiCad 库符号的命名约定——电源网名（`+3V3` / `3.3V` / `1V8` / `5V`）
 * 与 `VCC` / `VDD` 族。**不含** GND / VSS 族（地是回路，不是电源输入）。
 */
export function isPowerInputPinName(name: string | undefined): boolean {
  if (name === undefined) return false;
  const value = name.trim();
  if (/^[+-]?\d+(?:\.\d+)?V\d*$/iu.test(value)) return true;
  return /^(?:V(?:CC|DD|BAT|IN|BUS|EE|REF|DDA|DDIO|DDP|DD_?CORE)|AVDD|DVDD)$/iu.test(value);
}

/** 地网名（含模拟地 / 数字地 / 大地）。 */
export function isGroundNetName(name: string | undefined): boolean {
  if (name === undefined) return false;
  return /^(?:GND|AGND|DGND|GNDA|VSS|VSSA|0V|EARTH)$/iu.test(name.trim());
}

/** 非地的电源网名：复用电源输入引脚名的命名约定，再排除地网名。 */
export function isPowerNetName(name: string | undefined): boolean {
  return !isGroundNetName(name) && isPowerInputPinName(name);
}

/** 解析器给未命名网络起的名字（`Net-(R1-Pad2)`）：不是设计里写下的信号名。 */
export function isSynthesizedNetName(name: string | undefined): boolean {
  if (name === undefined) return true;
  const value = name.trim();
  return value.length === 0 || value.startsWith("Net-(");
}
