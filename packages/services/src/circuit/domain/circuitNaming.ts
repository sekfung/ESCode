import type { PageTerminal } from "./kicadConnectivity.js";

/** 元素命名与路径小工具：网络 id 必须跨「重新解析」稳定，才能让修正按 id 重新套用（§3.4）。 */

export interface PinRefKey {
  componentId: string;
  pin: string;
}

function sortedPinRefs(pinRefs: PinRefKey[]): PinRefKey[] {
  return [...pinRefs].sort(
    (a, b) => a.componentId.localeCompare(b.componentId) || a.pin.localeCompare(b.pin),
  );
}

function hashPrefix(value: string): string {
  let hash = 2166136261;
  for (const char of value) {
    hash ^= char.codePointAt(0)!;
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}

/**
 * 由名字生成元素 id（修正新建的网络 / 功能模块）：绑定源文件里的名字，重新解析后保持稳定。
 * `scope` 的语义同 makeNetId。
 */
export function makeNamedElementId(prefix: "net" | "block", name: string, scope?: string): string {
  const readable = name.replace(/[^\w.:+-]+/gu, "_").slice(0, 96);
  const suffixes: string[] = [];
  if (name.length > readable.length) suffixes.push(hashPrefix(`anchor:${name}`));
  if (scope) suffixes.push(hashPrefix(`scope:${scope}`));
  return `${prefix}:${readable}${suffixes.map((suffix) => `:${suffix}`).join("")}`;
}

/**
 * 网络 id：优先用源文件里的网络名（global / 电源 / hierarchy / local 标签），
 * 未命名网络用排序后的首个引脚做锚点；两种都只依赖源文件内容，重新解析后保持稳定。
 *
 * `scope` 只在「名字被限制在某张图纸实例内」时给出（local / hierarchy label）：不同图纸实例里
 * 同名的网络其实是不同的网，必须靠作用域区分开；global label 与电源符号不带 scope。
 */
export function makeNetId(name: string | undefined, pinRefs: PinRefKey[], scope?: string): string {
  if (name !== undefined && name.length > 0) return makeNamedElementId("net", name, scope);
  const anchor = sortedPinRefs(pinRefs)
    .slice(0, 1)
    .map((pinRef) => `${pinRef.componentId}.${pinRef.pin}`)
    .join();
  const readable = anchor.replace(/[^\w.:+-]+/gu, "_").slice(0, 96);
  const suffixes: string[] = [];
  if (anchor.length > readable.length) suffixes.push(hashPrefix(`anchor:${anchor}`));
  if (scope) suffixes.push(hashPrefix(`scope:${scope}`));
  return `net:${readable}${suffixes.map((suffix) => `:${suffix}`).join("")}`;
}

/** 网络命名优先级：global label > 电源符号 > hierarchy label > local label；同级按字典序取第一个。 */
export function pickNetName(terminals: PageTerminal[]): string | undefined {
  const priority = ["global-label", "power", "hierarchical-label", "label"] as const;
  for (const kind of priority) {
    const named = terminals
      .filter((terminal) => terminal.kind === kind && terminal.name)
      .map((terminal) => terminal.name!)
      .sort((a, b) => a.localeCompare(b));
    if (named[0]) return named[0];
  }
  return undefined;
}

export function dedupePinRefs(pinRefs: PinRefKey[]): PinRefKey[] {
  const seen = new Set<string>();
  return pinRefs.filter((pinRef) => {
    const key = `${pinRef.componentId}\u0000${pinRef.pin}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** 未命名网络的 KiCad 风格名字：`Net-(C1-Pad1)`（按位号 + 引脚号排序取第一个）。 */
export function synthesizeNetName(pins: { ref: string; pinNumber: string }[]): string | undefined {
  const sorted = [...pins].sort(
    (a, b) => a.ref.localeCompare(b.ref) || a.pinNumber.localeCompare(b.pinNumber),
  );
  const first = sorted[0];
  return first ? `Net-(${first.ref}-Pad${first.pinNumber})` : undefined;
}

export function dirName(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  const index = normalized.lastIndexOf("/");
  return index <= 0 ? "" : normalized.slice(0, index);
}

/** 子图纸路径解析：相对路径按父文件目录拼接并折叠 `.`/`..`；绝对路径原样返回（会落进 unresolved-sheet）。 */
export function joinWorkspacePath(directory: string, file: string): string {
  const normalizedFile = file.replaceAll("\\", "/");
  if (/^(?:\/|[A-Za-z]:\/)/u.test(normalizedFile)) return normalizedFile;
  const segments = [...directory.split("/").filter(Boolean), ...normalizedFile.split("/")];
  const stack: string[] = [];
  for (const segment of segments) {
    if (segment === "." || segment === "") continue;
    if (segment === "..") stack.pop();
    else stack.push(segment);
  }
  return stack.join("/");
}
