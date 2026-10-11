/**
 * 识别评测的比对口径（docs/circuit/README.md §10.4）：纯函数，零 IO，供离线评测脚本与单测使用。
 *
 * 匹配口径（写进报告，避免读数歧义）：
 * - 器件按「位号 + 值/型号」：位号必须相同；`value` / `partNumber` 只在两侧都给出来时比对
 *   （一侧缺失不算不匹配——模型不一定读得到丝印型号），去首尾空白后不区分大小写。
 * - 网络按「名称 + 引脚集合」：名称必须相同（两侧都匿名时按引脚集合匹配）；引脚集合写成
 *   `<位号>.<引脚号>` 的集合，双侧都取同一集合才算匹配；集合内顺序无关。
 * - 标准答案来自 `.kicad_sch` 的确定性解析（CircuitService），识别结果来自模型提交的草稿 JSON。
 */

/** 识别草稿里的一个器件（草稿 id 只在草稿内互引，比对时用位号）。 */
function componentKey(component) {
  return String(component?.ref ?? "")
    .trim()
    .toLowerCase();
}

function normalizeText(value) {
  return typeof value === "string" ? value.trim().toLowerCase() : undefined;
}

/** 器件相等：位号相同 + 两侧都给出的 value / partNumber 相同。 */
export function componentsMatch(standard, recognized) {
  if (componentKey(standard) !== componentKey(recognized)) return false;
  for (const field of ["value", "partNumber"]) {
    const left = normalizeText(standard?.[field]);
    const right = normalizeText(recognized?.[field]);
    if (left !== undefined && right !== undefined && left !== right) return false;
  }
  return true;
}

/** 网络相等：名称相同（都缺失时看引脚集合）+ 引脚集合相同。 */
export function netsMatch(standard, recognized) {
  const leftName = normalizeText(standard?.name);
  const rightName = normalizeText(recognized?.name);
  if (leftName !== undefined || rightName !== undefined) {
    if (leftName !== rightName) return false;
  }
  const left = pinSet(standard);
  const right = pinSet(recognized);
  return left.size === right.size && [...left].every((pin) => right.has(pin));
}

/** 引脚集合：`<位号>.<引脚号>`（标准答案用 KiCad UUID 作元素 id，识别结果用服务端 id，都必须换成位号）。 */
export function pinSet(net) {
  const pins = new Set();
  for (const pinRef of net?.pinRefs ?? []) {
    const ref = String(pinRef?.ref ?? "").trim();
    const pin = String(pinRef?.pin ?? "").trim();
    if (ref && pin) pins.add(`${ref}.${pin}`);
  }
  return pins;
}

/** 识别草稿 → 比对视图：草稿 id 换成人读的位号/引脚（草稿本身没有最终 id）。 */
export function comparisonView(draft) {
  const refByDraftId = new Map();
  for (const component of draft?.components ?? [])
    refByDraftId.set(component.id, String(component.ref ?? "").trim());
  return {
    components: (draft?.components ?? []).map((component) => ({
      ref: String(component.ref ?? "").trim(),
      value: component.value,
      partNumber: component.partNumber,
    })),
    nets: (draft?.nets ?? []).map((net) => ({
      name: net.name,
      pinRefs: (net.pinRefs ?? []).map((pinRef) => ({
        ref: refByDraftId.get(pinRef.componentId) ?? pinRef.componentId,
        pin: pinRef.pin,
      })),
    })),
  };
}

/** IR（标准答案）→ 比对视图：元素 id 换成位号，网络带上引脚集合。 */
export function standardView(ir) {
  const refById = new Map((ir?.components ?? []).map((component) => [component.id, component.ref]));
  return {
    components: (ir?.components ?? []).map((component) => ({
      ref: component.ref,
      value: component.value,
      partNumber: component.partNumber,
    })),
    nets: (ir?.nets ?? []).map((net) => ({
      name: net.name,
      pinRefs: (net.pinRefs ?? []).map((pinRef) => ({
        ref: refById.get(pinRef.componentId) ?? pinRef.componentId,
        pin: pinRef.pin,
      })),
    })),
  };
}

/**
 * 逐类比对：每个标准元素找第一个尚未匹配的识别元素（一对多/多对一都算未匹配）。
 * 返回准确率/召回率与两侧未匹配清单（报告里要列出「未能匹配的元素」）。
 */
export function compareElements(standardItems, recognizedItems, matcher) {
  const matchedRecognized = new Set();
  const missing = [];
  let matched = 0;
  for (const standardItem of standardItems) {
    const index = recognizedItems.findIndex(
      (candidate, candidateIndex) =>
        !matchedRecognized.has(candidateIndex) && matcher(standardItem, candidate),
    );
    if (index < 0) {
      missing.push(describe(standardItem));
      continue;
    }
    matchedRecognized.add(index);
    matched += 1;
  }
  const extra = recognizedItems.filter((_, index) => !matchedRecognized.has(index)).map(describe);
  return {
    matched,
    standard: standardItems.length,
    recognized: recognizedItems.length,
    precision: ratio(matched, recognizedItems.length),
    recall: ratio(matched, standardItems.length),
    missing,
    extra,
  };
}

/** 一次识别的完整比对结果（器件 + 网络）。 */
export function compareRecognition(standard, recognized) {
  return {
    components: compareElements(standard.components, recognized.components, componentsMatch),
    nets: compareElements(standard.nets, recognized.nets, netsMatch),
  };
}

/** 多份比对结果的合计（micro 平均：合计匹配数 / 合计元素数，不是各板块平均值的平均）。 */
export function aggregateResults(results) {
  const total = { components: emptyTotals(), nets: emptyTotals() };
  for (const result of results) {
    for (const kind of ["components", "nets"]) {
      total[kind].matched += result[kind].matched;
      total[kind].standard += result[kind].standard;
      total[kind].recognized += result[kind].recognized;
    }
  }
  return {
    components: {
      ...total.components,
      precision: ratio(total.components.matched, total.components.recognized),
      recall: ratio(total.components.matched, total.components.standard),
    },
    nets: {
      ...total.nets,
      precision: ratio(total.nets.matched, total.nets.recognized),
      recall: ratio(total.nets.matched, total.nets.standard),
    },
  };
}

function emptyTotals() {
  return { matched: 0, standard: 0, recognized: 0 };
}

function ratio(matched, total) {
  return total === 0 ? null : matched / total;
}

function describe(item) {
  const pins = pinSet(item);
  return pins.size > 0
    ? `${item.ref ?? item.name} (${[...pins].sort().join(", ")})`
    : String(item.ref ?? item.name ?? "");
}

/** 百分比显示：分母为 0 时给 `n/a`（该板块没有这类元素）。 */
export function formatRatio(value) {
  return value === null ? "n/a" : `${(value * 100).toFixed(1)}%`;
}
