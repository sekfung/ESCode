/**
 * 「引脚数与封装一致」的判定（docs/circuit/README.md §4）：
 * IR / 草稿里的 `footprint` 只是一串封装名（KiCad 的 `库:封装`），仓库里没有封装库可查期望引脚数，
 * 因此这里只做**确定性的封装编号解析**：认得出来就比对，认不出来就不判定（不写诊断、不扣分）。
 * 纯函数、零 IO；口径与限制见 packages/services/src/circuit/CONTRACT.md。
 */

/**
 * 白名单封装族：KiCad 封装库里这些族的命名是「族-引脚数」（如 `SOIC-8`、`TSSOP-16`、`QFN-32-1EP_…`、
 * `SOT-23-5`），数字部分就是引脚数。`SOT-23` 只认带显式引脚数的 `SOT-23-5` / `SOT-23-6`（裸 `SOT-23` 不判定）；
 * 刻意不收录语义有歧义的族（`SOT-223` 的 223 不是引脚数、`TO-220` 变体太多）。
 */
const PIN_COUNT_FAMILIES = [
  "BGA",
  "DFN",
  "DIP",
  "ETSSOP",
  "HTSSOP",
  "LGA",
  "LQFP",
  "MSOP",
  "PDIP",
  "PQFP",
  "QFN",
  "QFP",
  "SOIC",
  "SON",
  "SOP",
  "SOT-23",
  "SSOP",
  "TDFN",
  "TQFP",
  "TSSOP",
  "UQFN",
  "VQFN",
  "WLCSP",
  "WQFN",
  "WSON",
] as const;

/** `SOIC-8` / `TSSOP-16_4.4x5.0mm_…`：族名后紧跟的引脚数。 */
const FAMILY_PIN_COUNT = new RegExp(
  `^(?:${PIN_COUNT_FAMILIES.join("|")})-(\\d{1,3})(?:[_-]|$)`,
  "iu",
);
/** `PinHeader_1x04_P2.54mm_Vertical` / `PinSocket_2x05_…`：m 排 n 列，引脚数 = m × n。 */
const PIN_HEADER = /^(?:PinHeader|PinSocket)_(\d{1,2})x(\d{1,2})(?:[_-]|$)/iu;

/**
 * 从 footprint 名称解析期望引脚数；解析不出来返回 undefined（调用方不判定）。
 * 同时接受 `库:封装` 与裸封装名，只取 `:` 后的部分（KiCad 的封装名里不含 `:`）。
 */
export function footprintPinCount(footprint: string | undefined): number | undefined {
  const name = footprint?.trim();
  if (!name) return undefined;
  const local = name.slice(name.lastIndexOf(":") + 1).trim();
  const header = PIN_HEADER.exec(local);
  if (header) return Number(header[1]) * Number(header[2]);
  const match = FAMILY_PIN_COUNT.exec(local);
  if (!match) return undefined;
  const count = Number(match[1]);
  // 0 引脚不是封装语义（`SOP-0` 这类命名），按「判不出来」处理。
  return count > 0 ? count : undefined;
}
