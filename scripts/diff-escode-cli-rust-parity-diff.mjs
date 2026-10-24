// 差分垂直切片的差分层：归一化（每条规则登记在 evidence）、首差异定位（UTF-16 位置 + JSON 路径 +
// 双侧 sha256）、按登记表匹配已批准差异。
//
// 归一化纪律（docs/specs/rust-unified-host.md §4.1/§4.4）：只做显式登记的最小集合；禁止 trim、
// 合并消息、工具排序、事件重排等放宽手段。键序归一（json-key-order）单独登记，并且字节级差异
// 数量会同时写入 evidence/report，不静默丢弃。

import { createHash } from "node:crypto";

/**
 * 归一化规则（每条都随 evidence 写出；新增规则必须给出理由并被 review，见 §4.4）。
 * order 属性仅为可读性分组：text → 字段值级 → 序列化级。
 */
export const NORMALIZATION = [
  {
    id: "temp-root",
    order: "text",
    pattern: "两侧各自临时根路径（HOME/workspace/storage/数据目录）",
    replacement: "<ROOT>",
  },
  {
    id: "os-version",
    order: "text",
    pattern: "- OS Version: [^\\\\]+",
    replacement: "- OS Version: <OS>",
    reason:
      "系统内核版本随 runner/机器变化，不属于协议契约；Platform 与 Shell 仍保留用于跨平台判定",
  },
  {
    id: "uuid",
    order: "text",
    pattern: "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    replacement: "<UUID>",
  },
  {
    id: "iso-timestamp",
    order: "text",
    pattern: "\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(\\.\\d+)?Z?",
    replacement: "<ISO_TS>",
  },
  { id: "epoch-ms", order: "text", pattern: "\\b1[6-9]\\d{11}\\b", replacement: "<EPOCH_MS>" },
  {
    id: "storage-id",
    order: "field",
    pattern: '"databaseId":"[0-9a-f]{64}"',
    replacement: '"databaseId":"<STORAGE_ID>"',
    reason:
      "databaseId 是各自数据库路径的 sha256；两侧存储路径天然不同（fixture 环境差异），值不可跨侧比较；字段存在性与长度仍保留",
  },
  {
    id: "runtime-token-ordinal",
    order: "field",
    pattern: '"(subscriptionId|logicalFrameId|logEpoch)":"[^"]+"',
    replacement: "<TOKEN:n>（按首次出现序映射）",
    reason:
      "subscriptionId/logEpoch/logicalFrameId 由运行期随机生成，逐次运行都不同；按首次出现顺序映射保留结构（出现次数、复用模式），错位/数量差异仍可见",
  },
  {
    id: "runtime-id-token",
    order: "field",
    pattern: "(?:msg|sub|sess)_[a-z0-9]{6,8}(?=[_-])",
    replacement: "<RAND>",
    reason:
      "运行期标识符前缀里的短随机令牌（如 msg_<rand8>_<uuid>）逐次运行不同；只掩掉随机段，保持结构（前缀、分段数、UUID 段）",
  },
  {
    id: "timing-fields",
    order: "field",
    pattern: '"([A-Za-z][A-Za-z]*Ms)":\\d+',
    replacement: '"$1":"<MS>"',
    reason:
      "时长类字段（durationMs/activeMs/elapsedMs/initialFrameEncodeMs 等 *Ms）是计时事实，逐次运行不同；掩码为 <MS>。移除条件：某时长字段成为契约值（预算/断言）时单独保留",
  },
  {
    id: "json-key-order",
    order: "serialization",
    pattern: "每行 JSON 对象按键名排序后比较",
    replacement: "sorted-keys",
    reason:
      "Rust 侧 serde_json 的 Map 默认按键排序（BTreeMap）、TS 侧 JSON.stringify 按插入序；两侧消费者都做 JSON 解析，键序不是契约。字节级差异行数同时记录在 evidence 的 byteLevelDiffering，不静默丢弃",
  },
];

const TEXT_RULES = NORMALIZATION.filter((rule) => rule.order === "text");
const TOKEN_FIELDS = new Set(["subscriptionId", "logicalFrameId", "logEpoch"]);

function sortValue(value) {
  if (Array.isArray(value)) return value.map(sortValue);
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, sortValue(value[key])]),
  );
}

function applyTextFieldRules(line, sideRoot) {
  let out = sideRoot
    ? line.split(sideRoot).join("<ROOT>").split(sideRoot.replaceAll("\\", "\\\\")).join("<ROOT>")
    : line;
  for (const rule of TEXT_RULES.slice(1)) {
    out = out.replace(new RegExp(rule.pattern, "g"), rule.replacement);
  }
  out = out.replace(/"databaseId":"[0-9a-f]{64}"/g, '"databaseId":"<STORAGE_ID>"');
  out = out.replace(/(?:msg|sub|sess)_[a-z0-9]{6,8}(?=[_-])/g, "<RAND>");
  out = out.replace(/"([A-Za-z][A-Za-z]*Ms)":\d+/g, '"$1":"<MS>"');
  // 值位置上的裸占位符补引号：归一后的行必须仍是合法 JSON，否则按方法分流（流/进度/消息）会失败。
  return out.replace(/:(<(?:ROOT|UUID|ISO_TS|EPOCH_MS|STORAGE_ID)>)/g, ':"$1"');
}

/** 行级归一化：文本规则 → 字段值级规则（含按侧序映射的运行期标识符）→ 键序归一。 */
export function normalizeLine(line, sideRoot, tokenMap = new Map()) {
  const withTextRules = applyTextFieldRules(line, sideRoot);
  const withTokens = withTextRules.replace(/"([A-Za-z]+)":"([^"]+)"/g, (whole, key, value) => {
    if (!TOKEN_FIELDS.has(key)) return whole;
    // 同一字段内的第 n 个不同取值 → <TOKEN:n>（按字段独立编号，不同字段的取值不互相挤占序号）。
    const tokenKey = `${key}:${value}`;
    if (!tokenMap.has(tokenKey)) {
      const perField = [...tokenMap.keys()].filter((existing) => existing.startsWith(`${key}:`));
      tokenMap.set(tokenKey, perField.length + 1);
    }
    return `"${key}":"<TOKEN:${tokenMap.get(tokenKey)}>"`;
  });
  try {
    return JSON.stringify(sortValue(JSON.parse(withTokens)));
  } catch {
    // 非 JSON 行（原始日志/错误文本）保持原文。
    return withTokens;
  }
}

/** 键序归一之前的文本（用于统计字节级差异，不参与判定）。 */
export function textBeforeKeyOrder(line, sideRoot) {
  return applyTextFieldRules(line, sideRoot).replace(/"([A-Za-z]+)":"([^"]+)"/g, (whole, key) =>
    TOKEN_FIELDS.has(key) ? `"${key}":"<TOKEN>"` : whole,
  );
}

export const sha256 = (value) => createHash("sha256").update(value).digest("hex");

export function firstDifference(left, right) {
  const max = Math.min(left.length, right.length);
  for (let index = 0; index < max; index++) {
    if (left[index] !== right[index]) return index;
  }
  return max;
}

/** 找出两份 JSON 文本首个不同的 JSON 路径（解析失败返回 null，不强求）。 */
export function firstJsonPath(left, right) {
  let a;
  let b;
  try {
    a = JSON.parse(left);
    b = JSON.parse(right);
  } catch {
    return null;
  }
  const walk = (x, y, path) => {
    if (typeof x !== typeof y || x === null || y === null) return x === y ? null : path;
    if (typeof x !== "object") return Object.is(x, y) ? null : path;
    if (Array.isArray(x) !== Array.isArray(y)) return path;
    if (Array.isArray(x)) {
      if (x.length !== y.length) return `${path}.length`;
      for (let index = 0; index < x.length; index++) {
        const found = walk(x[index], y[index], `${path}[${index}]`);
        if (found) return found;
      }
      return null;
    }
    for (const key of [...new Set([...Object.keys(x), ...Object.keys(y)])].sort()) {
      const found = walk(x[key], y[key], `${path}.${key}`);
      if (found) return found;
    }
    return null;
  };
  return walk(a, b, "$");
}

function record(surface, index, a, b) {
  return {
    surface,
    index,
    left: a,
    right: b,
    offset: a !== null && b !== null ? firstDifference(a, b) : null,
    jsonPath: a !== null && b !== null ? firstJsonPath(a, b) : null,
    leftSha256: a === null ? null : sha256(a),
    rightSha256: b === null ? null : sha256(b),
  };
}

export function diffLines(surface, leftLines, rightLines) {
  const records = [];
  const max = Math.max(leftLines.length, rightLines.length);
  for (let index = 0; index < max; index++) {
    const a = leftLines[index] ?? null;
    const b = rightLines[index] ?? null;
    if (a === b) continue;
    records.push(record(surface, index, a, b));
  }
  return records;
}

/** 字节级（键序归一之前）差异行数：只用于诊断展示，不参与判定。 */
export function countByteLevelDifferences(leftLines, rightLines) {
  const max = Math.max(leftLines.length, rightLines.length);
  let count = 0;
  for (let index = 0; index < max; index++) {
    if ((leftLines[index] ?? null) !== (rightLines[index] ?? null)) count += 1;
  }
  return count;
}

/**
 * 基线指纹（未分类欠账登记用）：面 + 序号 + JSON 路径 + 双侧头部（数字串归一为 #）。
 * 数字归一是为了让 elapsedMs/计数类波动不产生新指纹；结构或文案变化仍会改变指纹 → 报为新差异。
 */
export function fingerprint(row) {
  const mask = (text) => (text ?? "").slice(0, 200).replace(/\d+/g, "#");
  return sha256(
    [row.surface, row.index, row.jsonPath ?? "-", mask(row.left), mask(row.right)].join("|"),
  );
}

export function matchApproved(scenario, row, entries) {
  return entries.find((entry) => {
    if (entry.scenario !== scenario || entry.surface !== row.surface) return false;
    const match = entry.match ?? {};
    if (match.index !== undefined && match.index !== row.index) return false;
    if (match.jsonPath !== undefined && match.jsonPath !== row.jsonPath) return false;
    if (match.containing !== undefined) {
      const text = row.left ?? row.right ?? "";
      if (!text.includes(match.containing)) return false;
    }
    // 双侧分别钉住（防止登记条目变成"未来任何变化都放行"的空白豁免）。
    if (match.leftContaining !== undefined && !(row.left ?? "").includes(match.leftContaining))
      return false;
    if (match.rightContaining !== undefined && !(row.right ?? "").includes(match.rightContaining))
      return false;
    if (match.leftSha256 !== undefined && row.leftSha256 !== match.leftSha256) return false;
    if (match.rightSha256 !== undefined && row.rightSha256 !== match.rightSha256) return false;
    return true;
  });
}

export function clip(text, offset, span = 160) {
  if (text === null) return null;
  const start = Math.max(0, (offset ?? 0) - 40);
  const slice = text.slice(start, start + span);
  return `${start > 0 ? "…" : ""}${slice}${text.length > start + span ? "…" : ""}`;
}
