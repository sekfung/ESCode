import type {
  CircuitBlock,
  CircuitComponent,
  CircuitDiagnostic,
  CircuitIR,
  CircuitIrSource,
  CircuitNet,
} from "@escode/shared/circuit";
import { makeNetId } from "./circuitNaming.js";
import { validateCircuitIr } from "./irValidation.js";
import type { BomDocument } from "./bom.js";
import type { KicadPcbDocument } from "./kicadPcb.js";
import type { KicadNetlistDocument } from "./netlist.js";
import type { KicadDesign } from "./kicadDesign.js";

/**
 * IR 组装：把各来源的确定性提取结果拼成 Circuit IR v1（§4）并跑一致性校验。
 * 解析得到的元素 confidence 恒为 1、by 恒为 "parser"（§2 规则 3）。
 */

export interface ParsedCircuitElements {
  components: CircuitComponent[];
  nets: CircuitNet[];
  blocks: CircuitBlock[];
  diagnostics: CircuitDiagnostic[];
}

const PARSER_ORIGIN = { by: "parser", confidence: 1 } as const;

export function assembleCircuitIr(input: {
  irId: string;
  revision: number;
  source: CircuitIrSource;
  stale: boolean;
  elements: ParsedCircuitElements;
}): CircuitIR {
  const ir: CircuitIR = {
    schemaVersion: 1,
    irId: input.irId,
    revision: input.revision,
    source: input.source,
    stale: input.stale,
    components: input.elements.components,
    nets: input.elements.nets,
    blocks: input.elements.blocks,
    diagnostics: [],
  };
  return revalidateCircuitIr({ ...ir, diagnostics: input.elements.diagnostics });
}

/** validateCircuitIr 会产出的诊断码；其余（如解析期的 pin-count-mismatch）在重算时保留。 */
const VALIDATION_DIAGNOSTIC_CODES = new Set([
  "duplicate-ref",
  "dangling-net",
  "single-pin-net",
  "unknown-component-reference",
  "unknown-net-reference",
]);

/** 修正或重新解析之后重算校验诊断：结构性诊断以最新状态为准，解析期诊断原样保留。 */
export function revalidateCircuitIr(ir: CircuitIR): CircuitIR {
  const preserved = ir.diagnostics.filter(
    (diagnostic) => !VALIDATION_DIAGNOSTIC_CODES.has(diagnostic.code),
  );
  return { ...ir, diagnostics: sortDiagnostics([...preserved, ...validateCircuitIr(ir)]) };
}

function sortDiagnostics(diagnostics: CircuitDiagnostic[]): CircuitDiagnostic[] {
  const unique = new Map<string, CircuitDiagnostic>();
  for (const diagnostic of diagnostics)
    unique.set(
      `${diagnostic.code}\u0000${diagnostic.componentIds ?? ""}\u0000${diagnostic.netIds ?? ""}\u0000${diagnostic.message}`,
      diagnostic,
    );
  return [...unique.values()].sort(
    (a, b) =>
      a.code.localeCompare(b.code) ||
      (a.componentIds?.[0] ?? "").localeCompare(b.componentIds?.[0] ?? "") ||
      (a.netIds?.[0] ?? "").localeCompare(b.netIds?.[0] ?? "") ||
      a.message.localeCompare(b.message),
  );
}

/** 原理图（层次展开 + 网络推导）已在 domain/kicadDesign.ts 完成。 */
export function elementsFromSchematicDesign(design: KicadDesign): ParsedCircuitElements {
  return {
    components: design.components,
    nets: design.nets,
    blocks: [],
    diagnostics: design.diagnostics,
  };
}

/**
 * `.kicad_pcb`：网络直接来自 pad 的 `(net <code> "<name>")`（文件里是显式的），
 * 没有 UUID 的元素用 footprint 的 uuid，KiCad 6 的 tstamp 由 kicadPcb.ts 统一处理。
 * `(net 0 ...)` 在 KiCad 里表示未连接，不作为网络。
 */
export function elementsFromPcb(pcb: KicadPcbDocument): ParsedCircuitElements {
  const connected = pcb.footprints.flatMap((footprint) =>
    footprint.pads
      .filter((pad) => pad.netName && pad.netName.length > 0 && pad.netNumber !== 0)
      .map((pad) => ({ footprint, pad, netName: pad.netName! })),
  );
  const netNames = [...new Set(connected.map((entry) => entry.netName))].sort();
  const netIdByName = new Map<string, string>();
  const nets: CircuitNet[] = [];
  for (const netName of netNames) {
    const pinRefs = connected
      .filter((entry) => entry.netName === netName)
      .map((entry) => ({ componentId: entry.footprint.id, pin: entry.pad.number }))
      .sort((a, b) => a.componentId.localeCompare(b.componentId) || a.pin.localeCompare(b.pin));
    const id = makeNetId(netName, pinRefs);
    netIdByName.set(netName, id);
    nets.push({ id, name: netName, pinRefs, origin: PARSER_ORIGIN });
  }
  const components: CircuitComponent[] = pcb.footprints.map((footprint) => ({
    id: footprint.id,
    ref: footprint.ref,
    ...(footprint.value ? { value: footprint.value } : {}),
    ...(footprint.footprint ? { footprint: footprint.footprint } : {}),
    pins: dedupePadsByNumber(footprint.pads).map((pad) => ({
      number: pad.number,
      ...(pad.netName && netIdByName.has(pad.netName) ? { netId: netIdByName.get(pad.netName) } : {}),
    })),
    origin: PARSER_ORIGIN,
  }));
  return { components, nets, blocks: [], diagnostics: [] };
}

/** 同一个编号可能有多个焊盘（散热焊盘等），IR 里一个引脚只出现一次（与 KiCad 网表一致）。 */
function dedupePadsByNumber<T extends { number: string }>(pads: T[]): T[] {
  const seen = new Set<string>();
  return pads.filter((pad) => {
    if (seen.has(pad.number)) return false;
    seen.add(pad.number);
    return true;
  });
}

/** KiCad 导出网表：没有 UUID，元素 id 用 `netlist:<位号>`（确定性、可重新解析后稳定）。 */
export function elementsFromNetlist(netlist: KicadNetlistDocument): ParsedCircuitElements {
  const componentId = (ref: string) => `netlist:${ref}`;
  const nets: CircuitNet[] = netlist.nets.map((net) => {
    const pinRefs = net.nodes
      .map((node) => ({ componentId: componentId(node.ref), pin: node.pin }))
      .sort((a, b) => a.componentId.localeCompare(b.componentId) || a.pin.localeCompare(b.pin));
    const name = net.name || undefined;
    return {
      id: makeNetId(name, pinRefs),
      ...(name ? { name } : {}),
      pinRefs,
      origin: PARSER_ORIGIN,
    };
  });
  const netIdByPin = new Map<string, string>();
  for (const net of nets)
    for (const pinRef of net.pinRefs)
      netIdByPin.set(`${pinRef.componentId}\u0000${pinRef.pin}`, net.id);
  const components: CircuitComponent[] = netlist.components
    .map((component) => {
      const id = componentId(component.ref);
      const pins = [
        ...new Set(
          netlist.nets.flatMap((net) =>
            net.nodes.filter((node) => componentId(node.ref) === id).map((node) => node.pin),
          ),
        ),
      ].sort();
      return {
        id,
        ref: component.ref,
        ...(component.value ? { value: component.value } : {}),
        ...(component.partNumber ? { partNumber: component.partNumber } : {}),
        ...(component.footprint ? { footprint: component.footprint } : {}),
        pins: pins.map((pin) => {
          const netId = netIdByPin.get(`${id}\u0000${pin}`);
          return { number: pin, ...(netId ? { netId } : {}) };
        }),
        origin: PARSER_ORIGIN,
      };
    })
    .sort((a, b) => a.ref.localeCompare(b.ref));
  return { components, nets, blocks: [], diagnostics: [] };
}

/** BOM：只有器件清单，没有连接信息 → nets 为空（面板显示 BOM 视图）。 */
export function elementsFromBom(bom: BomDocument): ParsedCircuitElements {
  const components: CircuitComponent[] = [];
  for (const row of bom.rows)
    for (const ref of row.refs)
      components.push({
        id: `bom:${ref}`,
        ref,
        ...(row.value ? { value: row.value } : {}),
        ...(row.partNumber ? { partNumber: row.partNumber } : {}),
        ...(row.footprint ? { footprint: row.footprint } : {}),
        pins: [],
        origin: PARSER_ORIGIN,
      });
  components.sort((a, b) => a.ref.localeCompare(b.ref));
  return { components, nets: [], blocks: [], diagnostics: [] };
}
