/**
 * KiCad s-expression 读取器（.kicad_sch / .kicad_pcb / 导出的网表都是这种括号语法）。
 * 纯函数、零 IO：输入文本，输出树；上层再按 head 取字段。
 */

export interface SExprList {
  readonly kind: "list";
  readonly items: readonly SExpr[];
}

export interface SExprAtom {
  readonly kind: "atom";
  readonly value: string;
}

export type SExpr = SExprList | SExprAtom;

const WHITESPACE = new Set([" ", "\t", "\r", "\n"]);
const ATOM_END = new Set([" ", "\t", "\r", "\n", "(", ")"]);

/** 解析失败时的错误：带上出错位置，便于定位损坏或非 s-expression 的源文件。 */
export class SExprParseError extends Error {
  constructor(
    message: string,
    readonly offset: number,
  ) {
    super(`${message}（offset ${offset}）`);
    this.name = "SExprParseError";
  }
}

/**
 * 解析一个 s-expression 文档。整个文档必须恰好是一个括号列表
 * （KiCad 文件以 `(kicad_sch ...)` / `(kicad_pcb ...)` 开头，网表以 `(export ...)` 开头）。
 */
export function parseSExpr(text: string): SExprList {
  const root: { kind: "list"; items: SExpr[] } = { kind: "list", items: [] };
  const stack: { kind: "list"; items: SExpr[] }[] = [root];
  let index = 0;
  while (index < text.length) {
    const char = text[index]!;
    if (WHITESPACE.has(char)) {
      index += 1;
      continue;
    }
    if (char === "(") {
      const next = { kind: "list" as const, items: [] as SExpr[] };
      stack.at(-1)!.items.push(next);
      stack.push(next);
      index += 1;
      continue;
    }
    if (char === ")") {
      if (stack.length === 1) throw new SExprParseError("多余的右括号", index);
      stack.pop();
      index += 1;
      continue;
    }
    if (char === '"') {
      const parsed = parseQuotedAtom(text, index);
      stack.at(-1)!.items.push({ kind: "atom", value: parsed.value });
      index = parsed.nextIndex;
      continue;
    }
    const start = index;
    while (index < text.length && !ATOM_END.has(text[index]!)) index += 1;
    stack.at(-1)!.items.push({ kind: "atom", value: text.slice(start, index) });
  }
  if (stack.length !== 1) throw new SExprParseError("括号没有闭合", text.length);
  const top = root.items;
  if (top.length !== 1 || top[0]!.kind !== "list")
    throw new SExprParseError("文档不是单个括号列表", 0);
  return top[0]!;
}

function parseQuotedAtom(text: string, start: number): { value: string; nextIndex: number } {
  let index = start + 1;
  let value = "";
  while (index < text.length) {
    const char = text[index]!;
    if (char === "\\") {
      const next = text[index + 1];
      if (next === undefined) break;
      // KiCad 只转义引号与反斜杠；其余保持原样，避免改写值里的路径分隔符。
      value += next === '"' || next === "\\" ? next : `\\${next}`;
      index += 2;
      continue;
    }
    if (char === '"') return { value, nextIndex: index + 1 };
    value += char;
    index += 1;
  }
  throw new SExprParseError("字符串没有闭合", start);
}

/** 列表的第一个原子，例如 `(symbol "R" ...)` 的 head 是 `symbol`。 */
export function listHead(node: SExpr): string | undefined {
  if (node.kind !== "list") return undefined;
  const first = node.items[0];
  return first?.kind === "atom" ? first.value : undefined;
}

/** 直接子列表（不递归）。 */
export function childLists(node: SExpr): SExprList[] {
  if (node.kind !== "list") return [];
  return node.items.filter((item): item is SExprList => item.kind === "list");
}

/** 按 head 过滤直接子列表：`childLists(list, "pin")`。 */
export function childListsOf(node: SExpr, head: string): SExprList[] {
  return childLists(node).filter((item) => listHead(item) === head);
}

export function firstChild(node: SExpr, head: string): SExprList | undefined {
  return childListsOf(node, head)[0];
}

/** 第 index 个参数原子（跳过 head）；越界或类型不符返回 undefined。 */
export function atomAt(node: SExpr, index: number): string | undefined {
  if (node.kind !== "list") return undefined;
  const item = node.items[index];
  return item?.kind === "atom" ? item.value : undefined;
}

/** 把参数原子按位置读出成数组，遇到子列表则截断（KiCad 的坐标、UUID 都写在 head 后面）。 */
export function atomsAfterHead(node: SExpr): string[] {
  if (node.kind !== "list") return [];
  const atoms: string[] = [];
  for (const item of node.items.slice(1)) {
    if (item.kind !== "atom") break;
    atoms.push(item.value);
  }
  return atoms;
}

export function numberOf(node: SExpr | undefined, index: number): number | undefined {
  if (!node) return undefined;
  const value = atomAt(node, index);
  if (value === undefined) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** 读取 `(at x y [rot])` 之类的字段。 */
export function readPoint(
  node: SExpr | undefined,
  index = 1,
): { x: number; y: number; rotation?: number } | undefined {
  if (!node) return undefined;
  const x = numberOf(node, index);
  const y = numberOf(node, index + 1);
  if (x === undefined || y === undefined) return undefined;
  const rotation = numberOf(node, index + 2);
  return rotation === undefined ? { x, y } : { x, y, rotation };
}

/** `(property "Reference" "R1" ...)` → { Reference: "R1" }。 */
export function readProperties(node: SExpr): Map<string, string> {
  const properties = new Map<string, string>();
  for (const property of childListsOf(node, "property")) {
    const name = atomAt(property, 1);
    const value = atomAt(property, 2);
    if (name !== undefined && value !== undefined) properties.set(name, value);
  }
  return properties;
}
