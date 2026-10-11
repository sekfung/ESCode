/**
 * KiCad 导出的网表解析（s-expression，KiCad 6+ 的 `(export …)` 格式）。
 *
 * 结构：
 * ```
 * (export (version "E")
 *   (components (comp (ref "C1") (value "100n") (footprint "…") (fields (field (name "MPN") "…"))))
 *   (nets (net (code "1") (name "GND") (node (ref "C1") (pin "1")) …)))
 * ```
 *
 * 版本差异依据：
 * - 自定义字段：KiCad 6 放在 `(fields (field (name "MPN") "值"))`；KiCad 7/8 的 comp 还会直接写
 *   `(property "MPN" "值")`（两种都出现在真实导出里），取值优先 property。
 * - node：KiCad 6 只有 ref/pin；KiCad 7/8 追加 `(pinfunction …)`、`(pintype …)`，这里只取 ref/pin。
 * - 网表里没有 UUID，元素 id 由上层（CircuitService）生成，本解析器只管字段。
 *
 * 纯函数、零 IO。顶层 head 不是 `export` 时抛 `unsupported_format`
 * （KiCad 5 及更早的旧 `.net` 文本不是本格式，解析失败时同样给这一错误码）。
 */

import {
  atomAt,
  childListsOf,
  firstChild,
  listHead,
  parseSExpr,
  readProperties,
  type SExpr,
  type SExprList,
} from "./sexpr.js";

export interface KicadNetlistDocument {
  components: { ref: string; value?: string; footprint?: string; partNumber?: string }[];
  nets: { name: string; code?: string; nodes: { ref: string; pin: string }[] }[];
}

const PART_NUMBER_FIELD = "MPN";

export function parseKicadNetlist(text: string): KicadNetlistDocument {
  const root = parseNetlistDocument(text);
  return {
    components: childListsOf(root, "components")
      .flatMap((group) => childListsOf(group, "comp"))
      .map(readComponent),
    nets: childListsOf(root, "nets")
      .flatMap((group) => childListsOf(group, "net"))
      .map(readNet),
  };
}

function parseNetlistDocument(text: string): SExprList {
  let root: SExprList;
  try {
    root = parseSExpr(text);
  } catch (error) {
    // KiCad 5 及更早的 `.net` 是另一套文本格式，解析器会先失败；统一改写成 unsupported_format。
    throw new Error(`unsupported_format: 不是 s-expression 网表：${errorMessage(error)}`);
  }
  const head = listHead(root);
  if (head !== "export") {
    throw new Error(`unsupported_format: 顶层 head 为 ${head ?? "(非列表)"}，期望 export`);
  }
  return root;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function readComponent(node: SExprList): KicadNetlistDocument["components"][number] {
  return {
    // 位号是 IR 组件的必需字段（docs/circuit/README.md §4），缺失时填空串保留元素供上层诊断。
    ref: firstNonEmpty(childAtom(node, "ref")) ?? "",
    value: nonEmpty(childAtom(node, "value")),
    footprint: nonEmpty(childAtom(node, "footprint")),
    partNumber: readPartNumber(node),
  };
}

function readNet(node: SExprList): KicadNetlistDocument["nets"][number] {
  return {
    name: firstNonEmpty(childAtom(node, "name")) ?? "",
    code: nonEmpty(childAtom(node, "code")),
    nodes: childListsOf(node, "node").map((child) => ({
      // 只保留 ref/pin；KiCad 7/8 的 pinfunction/pintype 不在 IR 的节点定义里（§4 的 pinRefs）。
      ref: firstNonEmpty(childAtom(child, "ref")) ?? "",
      pin: firstNonEmpty(childAtom(child, "pin")) ?? "",
    })),
  };
}

/** MPN：优先 comp 的 `(property "MPN" …)`，其次 `(fields (field (name "MPN") "…"))`，都没有时省略。 */
function readPartNumber(node: SExprList): string | undefined {
  const fromProperty = readProperty(node, PART_NUMBER_FIELD);
  if (fromProperty !== undefined) return fromProperty;
  for (const fields of childListsOf(node, "fields")) {
    for (const field of childListsOf(fields, "field")) {
      const name = childAtom(field, "name");
      if (name === undefined || name.trim().toUpperCase() !== PART_NUMBER_FIELD) continue;
      const value = nonEmpty(firstValueAtom(field));
      if (value !== undefined) return value;
    }
  }
  return undefined;
}

/**
 * 字段值写在 `(name "MPN")` 子列表之后（`(field (name "MPN") "RC0402FR-0710KL")`），
 * 所以取 head 之后第一个直接原子，而不是固定下标。
 */
function firstValueAtom(node: SExpr): string | undefined {
  if (node.kind !== "list") return undefined;
  for (const item of node.items.slice(1)) {
    if (item.kind === "atom") return item.value;
  }
  return undefined;
}

/** 按字段名取 property，大小写与首尾空白不敏感（MPN/mpn 都是常见写法）。 */
function readProperty(node: SExprList, name: string): string | undefined {
  const properties = readProperties(node);
  const exact = nonEmpty(properties.get(name));
  if (exact !== undefined) return exact;
  const normalized = name.toUpperCase();
  for (const [key, value] of properties) {
    if (key.trim().toUpperCase() === normalized) return nonEmpty(value);
  }
  return undefined;
}

/** 读取 `(head "值")` 形式的直接子节点，例如 `(ref "C1")`、`(pin "1")`。 */
function childAtom(node: SExpr, head: string): string | undefined {
  const child = firstChild(node, head);
  return child === undefined ? undefined : atomAt(child, 1);
}

/** KiCad 会把没有值的字段写成空串（如 `(name "")`）；空串按“没有该字段”处理。 */
function nonEmpty(value: string | undefined): string | undefined {
  return value !== undefined && value.length > 0 ? value : undefined;
}

function firstNonEmpty(...values: (string | undefined)[]): string | undefined {
  return values.find((value) => value !== undefined && value.length > 0);
}
