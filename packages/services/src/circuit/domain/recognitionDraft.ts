import type {
  CircuitBlock,
  CircuitComponent,
  CircuitDiagnostic,
  CircuitDiagnosticCode,
  CircuitNet,
  CircuitOrigin,
  CircuitRecognitionDraft,
  CircuitRegion,
} from "@escode/shared/circuit";
import {
  dedupePinRefs,
  makeNamedElementId,
  makeNetId,
  makeRecognitionComponentId,
} from "./circuitNaming.js";
import { recognitionConfidence } from "./recognitionConfidence.js";
import { evidenceHasText, type CircuitVectorEvidence } from "./recognitionEvidence.js";
import { footprintPinCount } from "./recognitionFootprint.js";
import { validateCircuitIr } from "./irValidation.js";

/**
 * 识别草稿 → IR 元素（docs/circuit/README.md §4、§5 circuit_submit_recognition）：
 * 1. 服务端生成最终元素 id（草稿 id 只作草稿内引用），`pinRefs` / `componentIds` / `pins[].netId` 同步改写；
 * 2. 跑 §4 的一致性校验（引脚数与封装一致、无重复位号、无单引脚网络、网络两端的器件都存在）并写 diagnostics；
 * 3. 置信度只能由服务端给出：矢量 PDF 文本证据命中的元素按确定性解析记 1，其余按校验结果扣分（基准 0.9）。
 * 证据命中优先于扣分（§2 规则 3 把可直接提取的文本算作确定性结果）。纯函数、零 IO。
 */

export interface RecognitionBuildInput {
  draft: CircuitRecognitionDraft;
  /** 矢量 PDF 的确定性证据；image 来源或只有位图的 PDF 没有。 */
  evidence?: CircuitVectorEvidence;
}

export interface RecognitionBuildResult {
  components: CircuitComponent[];
  nets: CircuitNet[];
  blocks: CircuitBlock[];
  diagnostics: CircuitDiagnostic[];
  /** 拿到文本证据、按确定性解析记 1 的元素数（进日志，便于判断 PDF 是否矢量）。 */
  evidenceHits: number;
}

const RECOGNITION_ORIGIN: CircuitOrigin = { by: "recognition", confidence: 0.9 };

/**
 * 草稿里的元素 id 只用于草稿内部互相引用；指向不存在的草稿 id 时原样保留，
 * 让 `validateCircuitIr` 报出 `unknown-component-reference` / `unknown-net-reference`（−0.3）。
 */
function remapId(id: string, idByDraftId: Map<string, string>): string {
  return idByDraftId.get(id) ?? id;
}

/** 同一 id 已被占用时追加 `#n`（n 从 2 起），保证 id 唯一且对同一草稿稳定。 */
function uniqueId(base: string, used: Set<string>, startOccurrence = 1): string {
  let occurrence = startOccurrence === 1 && !used.has(base) ? 1 : Math.max(startOccurrence, 2);
  while (used.has(occurrence === 1 ? base : `${base}#${occurrence}`)) occurrence += 1;
  const id = occurrence === 1 ? base : `${base}#${occurrence}`;
  used.add(id);
  return id;
}

export function buildRecognitionElements(input: RecognitionBuildInput): RecognitionBuildResult {
  const { draft, evidence } = input;
  const componentIdByDraftId = new Map<string, string>();
  const netIdByDraftId = new Map<string, string>();
  /** 最终元素 id → 草稿给的 region（面板据此在原图上叠识别框，§8）。 */
  const regionByElementId = new Map<string, CircuitRegion>();
  const usedNetIds = new Set<string>();
  const usedBlockIds = new Set<string>();

  // 三趟：先把器件 id 定下来（网络要用它重写 pinRefs），再定网络 id，最后才建器件
  // （器件的 pins[].netId 必须等到网络 id 全部就绪后再重写，否则会被误判成悬空引用）。
  const componentIdsByIndex: string[] = [];
  const refOccurrences = new Map<string, number>();
  for (const component of draft.components) {
    const occurrence = (refOccurrences.get(component.ref) ?? 0) + 1;
    refOccurrences.set(component.ref, occurrence);
    const id = makeRecognitionComponentId(component.ref, occurrence);
    componentIdByDraftId.set(component.id, id);
    componentIdsByIndex.push(id);
    if (component.origin.region) regionByElementId.set(id, component.origin.region);
  }

  const nets: CircuitNet[] = draft.nets.map((net) => {
    const pinRefs = dedupePinRefs(
      net.pinRefs.map((pinRef) => ({
        componentId: remapId(pinRef.componentId, componentIdByDraftId),
        pin: pinRef.pin,
      })),
    );
    // 网络 id 与确定性解析共用同一套派生规则（名字优先、否则用首个引脚锚点）；
    // 只有同名且引脚集也相同的重复网络才需要序号后缀，保证 id 唯一。
    const id = uniqueId(makeNetId(net.name, pinRefs), usedNetIds);
    netIdByDraftId.set(net.id, id);
    if (net.origin.region) regionByElementId.set(id, net.origin.region);
    return {
      id,
      ...(net.name !== undefined ? { name: net.name } : {}),
      pinRefs,
      origin: RECOGNITION_ORIGIN,
    };
  });

  const components: CircuitComponent[] = draft.components.map((component, index) => ({
    id: componentIdsByIndex[index]!,
    ref: component.ref,
    ...(component.value !== undefined ? { value: component.value } : {}),
    ...(component.partNumber !== undefined ? { partNumber: component.partNumber } : {}),
    ...(component.footprint !== undefined ? { footprint: component.footprint } : {}),
    pins: component.pins.map((pin) => ({
      number: pin.number,
      ...(pin.name !== undefined ? { name: pin.name } : {}),
      ...(pin.netId !== undefined ? { netId: remapId(pin.netId, netIdByDraftId) } : {}),
    })),
    origin: RECOGNITION_ORIGIN,
  }));

  const blocks: CircuitBlock[] = [];
  const blockNameOccurrences = new Map<string, number>();
  for (const block of draft.blocks ?? []) {
    const occurrence = (blockNameOccurrences.get(block.name) ?? 0) + 1;
    blockNameOccurrences.set(block.name, occurrence);
    const id = uniqueId(makeNamedElementId("block", block.name), usedBlockIds, occurrence);
    if (block.origin.region) regionByElementId.set(id, block.origin.region);
    blocks.push({
      id,
      name: block.name,
      componentIds: block.componentIds.map((componentId) =>
        remapId(componentId, componentIdByDraftId),
      ),
      origin: RECOGNITION_ORIGIN,
    });
  }

  const diagnostics = [
    ...pinCountDiagnostics(components),
    ...validateCircuitIr({ components, nets, blocks }),
  ];
  const componentIds = new Set(components.map((component) => component.id));
  const origins = recognitionOrigins({
    components,
    nets,
    blocks,
    componentIds,
    diagnostics,
    evidence,
    regionByElementId,
  });
  const evidenceHits = [...origins.values()].filter((origin) => origin.by === "parser").length;
  const withOrigin = <T extends { id: string }>(element: T): T => ({
    ...element,
    origin: origins.get(element.id) ?? RECOGNITION_ORIGIN,
  });
  return {
    components: components.map(withOrigin),
    nets: nets.map(withOrigin),
    blocks: blocks.map(withOrigin),
    diagnostics,
    evidenceHits,
  };
}

/**
 * 「引脚数与封装一致」（§4）：只在该器件的 footprint 能解析出封装编号时比对，
 * 解析不出来就不断言（见 domain/recognitionFootprint.ts）。判定不通过时写 warning，扣 0.2。
 */
function pinCountDiagnostics(components: CircuitComponent[]): CircuitDiagnostic[] {
  const diagnostics: CircuitDiagnostic[] = [];
  for (const component of components) {
    const expected = footprintPinCount(component.footprint);
    if (expected === undefined || expected === component.pins.length) continue;
    diagnostics.push({
      code: "pin-count-mismatch",
      severity: "warning",
      message: `器件 ${component.ref} 有 ${component.pins.length} 个引脚，封装 ${component.footprint} 应为 ${expected} 个`,
      componentIds: [component.id],
    });
  }
  return diagnostics;
}

/**
 * 元素来源与置信度：
 * - 矢量 PDF 文本证据命中（器件的位号、网络的名称）→ `by: "parser"`、`confidence: 1`（§2 规则 3）；
 * - 其余元素 → `by: "recognition"`、按涉及自己的诊断类别扣分（基准 0.9，夹在 [0.05, 0.9]）；
 * - `region` 一律从草稿带进 `origin.region`（面板据此在原图上叠识别框，§8）。
 * 模块（blocks）没有可匹配的文本，只按「引用了不存在的器件」扣分；IR 的诊断把缺失 id 记在
 * `componentIds` 上（irValidation.ts），所以这里直接判定而不是从诊断反查。
 */
function recognitionOrigins(input: {
  components: CircuitComponent[];
  nets: CircuitNet[];
  blocks: CircuitBlock[];
  componentIds: ReadonlySet<string>;
  diagnostics: CircuitDiagnostic[];
  evidence?: CircuitVectorEvidence;
  regionByElementId: ReadonlyMap<string, CircuitRegion>;
}): Map<string, CircuitOrigin> {
  const codesByElement = new Map<string, Set<CircuitDiagnosticCode>>();
  const record = (id: string, code: CircuitDiagnosticCode) => {
    const codes = codesByElement.get(id);
    if (codes) codes.add(code);
    else codesByElement.set(id, new Set([code]));
  };
  for (const diagnostic of input.diagnostics)
    for (const id of [...(diagnostic.componentIds ?? []), ...(diagnostic.netIds ?? [])])
      record(id, diagnostic.code);

  const origins = new Map<string, CircuitOrigin>();
  for (const component of input.components) {
    const region = input.regionByElementId.get(component.id);
    origins.set(
      component.id,
      evidenceHasText(input.evidence, region, [component.ref])
        ? parserOrigin(region)
        : recognitionOrigin(codesByElement.get(component.id), region),
    );
  }
  for (const net of input.nets) {
    const region = input.regionByElementId.get(net.id);
    origins.set(
      net.id,
      evidenceHasText(input.evidence, region, net.name ? [net.name] : [])
        ? parserOrigin(region)
        : recognitionOrigin(codesByElement.get(net.id), region),
    );
  }
  for (const block of input.blocks) {
    const codes = new Set(codesByElement.get(block.id));
    if (block.componentIds.some((componentId) => !input.componentIds.has(componentId)))
      codes.add("unknown-component-reference");
    origins.set(block.id, recognitionOrigin(codes, input.regionByElementId.get(block.id)));
  }
  return origins;
}

function parserOrigin(region: CircuitRegion | undefined): CircuitOrigin {
  return { by: "parser", confidence: 1, ...(region ? { region } : {}) };
}

function recognitionOrigin(
  codes: ReadonlySet<CircuitDiagnosticCode> | undefined,
  region: CircuitRegion | undefined,
): CircuitOrigin {
  const { confidence } = recognitionConfidence(codes ?? []);
  return { by: "recognition", confidence, ...(region ? { region } : {}) };
}
