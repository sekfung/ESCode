import type {
  CircuitBlock,
  CircuitComponent,
  CircuitCorrectionOp,
  CircuitIR,
  CircuitNet,
  CircuitOrigin,
} from "@escode/shared/circuit";
import { CircuitServiceError } from "./circuitErrors.js";
import { makeNamedElementId } from "./circuitNaming.js";

/**
 * 修正的套用与重新套用（§3.3、§3.4、§8）。
 *
 * - 修正只改 IR 结构：改位号/型号/值、增删连接、增删功能模块；**不能**新增或删除器件（P1 非目标）。
 * - 同一引脚只能在一个网络上：connect 会把引脚从原网络移走（KiCad 的语义），disconnect 之后引脚悬空。
 * - 重新解析后按目标 id 重新套用：目标（器件 UUID / 网络 id / 模块 id）找不到就标记 orphaned，
 *   不猜、不迁移到别的元素。
 */

export interface CorrectionTarget {
  kind: "element" | "net" | "block";
  id: string;
  pin?: string;
}

export interface CircuitCorrectionRecord {
  /** 应用该修正后产生的 revision。 */
  revision: number;
  appliedAt: string;
  author: { kind: "user" | "agent"; sessionId?: string };
  target: CorrectionTarget;
  op: CircuitCorrectionOp;
  /** 重新解析后套用不上的修正（§3.4）：保留记录、在面板里列出，等用户确认或删除。 */
  orphaned?: boolean;
}

export interface AppliedCorrection {
  op: CircuitCorrectionOp;
  target: CorrectionTarget;
}

export function correctionTargetOf(op: CircuitCorrectionOp): CorrectionTarget {
  switch (op.op) {
    case "update-component":
      return { kind: "element", id: op.componentId };
    case "connect":
    case "disconnect":
      return { kind: "element", id: op.componentId, pin: op.pin };
    case "update-block":
    case "remove-block":
      return { kind: "block", id: op.blockId };
    case "add-block":
      return { kind: "block", id: makeNamedElementId("block", op.name) };
  }
}

/**
 * 修正后的元素来源：人工修正视为确定性事实（confidence 1），Agent 修正保留原置信度
 * （§2 规则 2 不让 Agent 把置信度推到 1）。这条规则在 CONTRACT.md 里记为待定项。
 */
function correctionOrigin(
  previous: CircuitOrigin | undefined,
  author: { kind: "user" | "agent"; sessionId?: string },
): CircuitOrigin {
  return {
    by: "correction",
    confidence: author.kind === "user" ? 1 : (previous?.confidence ?? 0.5),
    author: { kind: author.kind, ...(author.sessionId ? { sessionId: author.sessionId } : {}) },
  };
}

export function applyCircuitCorrections(
  ir: CircuitIR,
  ops: readonly CircuitCorrectionOp[],
  author: { kind: "user" | "agent"; sessionId?: string },
): { ir: CircuitIR; applied: AppliedCorrection[] } {
  const components: CircuitComponent[] = ir.components.map((component) => ({
    ...component,
    pins: component.pins.map((pin) => ({ ...pin })),
  }));
  const nets: CircuitNet[] = ir.nets.map((net) => ({
    ...net,
    pinRefs: net.pinRefs.map((pinRef) => ({ ...pinRef })),
  }));
  const blocks: CircuitBlock[] = ir.blocks.map((block) => ({
    ...block,
    componentIds: [...block.componentIds],
  }));
  const componentById = new Map(components.map((component) => [component.id, component]));
  const netById = new Map(nets.map((net) => [net.id, net]));
  const applied: AppliedCorrection[] = [];

  const requireComponent = (id: string): CircuitComponent => {
    const component = componentById.get(id);
    if (!component) throw new CircuitServiceError("invalid_ir", `修正引用了不存在的器件 ${id}`);
    return component;
  };
  const requirePin = (component: CircuitComponent, pinNumber: string) => {
    const pin = component.pins.find((candidate) => candidate.number === pinNumber);
    if (!pin)
      throw new CircuitServiceError(
        "invalid_ir",
        `修正引用了器件 ${component.ref} 上不存在的引脚 ${pinNumber}`,
      );
    return pin;
  };

  for (const op of ops) {
    switch (op.op) {
      case "update-component": {
        const component = requireComponent(op.componentId);
        if (op.ref !== undefined) component.ref = op.ref;
        if (op.value !== undefined) component.value = op.value;
        if (op.partNumber !== undefined) component.partNumber = op.partNumber;
        if (op.footprint !== undefined) component.footprint = op.footprint;
        component.origin = correctionOrigin(component.origin, author);
        break;
      }
      case "connect": {
        const component = requireComponent(op.componentId);
        const pin = requirePin(component, op.pin);
        let net: CircuitNet | undefined;
        if (op.netId !== undefined) {
          net = netById.get(op.netId);
          if (!net) throw new CircuitServiceError("invalid_ir", `修正引用了不存在的网络 ${op.netId}`);
        }
        if (net === undefined && op.netName !== undefined) {
          const named = nets
            .filter((candidate) => candidate.name === op.netName)
            .sort((a, b) => a.id.localeCompare(b.id))[0];
          if (named) net = named;
          else {
            net = {
              id: makeNamedElementId("net", op.netName),
              name: op.netName,
              pinRefs: [],
              origin: correctionOrigin(undefined, author),
            };
            nets.push(net);
            netById.set(net.id, net);
          }
        }
        if (!net) throw new CircuitServiceError("invalid_ir", "connect 需要 netId 或 netName");
        for (const candidate of nets)
          if (candidate.id !== net.id)
            candidate.pinRefs = candidate.pinRefs.filter(
              (pinRef) => !(pinRef.componentId === component.id && pinRef.pin === op.pin),
            );
        if (!net.pinRefs.some((pinRef) => pinRef.componentId === component.id && pinRef.pin === op.pin))
          net.pinRefs.push({ componentId: component.id, pin: op.pin });
        net.pinRefs.sort(
          (a, b) => a.componentId.localeCompare(b.componentId) || a.pin.localeCompare(b.pin),
        );
        net.origin = correctionOrigin(net.origin, author);
        pin.netId = net.id;
        break;
      }
      case "disconnect": {
        const component = requireComponent(op.componentId);
        const pin = requirePin(component, op.pin);
        if (pin.netId !== undefined) {
          const net = netById.get(pin.netId);
          if (net) {
            net.pinRefs = net.pinRefs.filter(
              (pinRef) => !(pinRef.componentId === component.id && pinRef.pin === op.pin),
            );
            net.origin = correctionOrigin(net.origin, author);
          }
          delete pin.netId;
        }
        break;
      }
      case "add-block":
      case "update-block": {
        const blockId =
          op.op === "add-block" ? makeNamedElementId("block", op.name) : op.blockId;
        const existing = blocks.find((block) => block.id === blockId);
        const name = op.name ?? existing?.name;
        const componentIds = op.componentIds ?? existing?.componentIds ?? [];
        if (name === undefined)
          throw new CircuitServiceError("invalid_ir", `修正引用了不存在的模块 ${blockId}`);
        for (const componentId of componentIds) requireComponent(componentId);
        if (existing) {
          existing.name = name;
          existing.componentIds = [...componentIds].sort();
          existing.origin = correctionOrigin(existing.origin, author);
        } else
          blocks.push({
            id: blockId,
            name,
            componentIds: [...componentIds].sort(),
            origin: correctionOrigin(undefined, author),
          });
        break;
      }
      case "remove-block": {
        const index = blocks.findIndex((block) => block.id === op.blockId);
        if (index < 0)
          throw new CircuitServiceError("invalid_ir", `修正引用了不存在的模块 ${op.blockId}`);
        blocks.splice(index, 1);
        break;
      }
    }
    applied.push({ op, target: correctionTargetOf(op) });
  }
  return { ir: { ...ir, components, nets, blocks }, applied };
}

/** 重新解析后重新套用历史修正；套用不上的保留记录并标记 orphaned（§3.4）。 */
export function reapplyCircuitCorrections(
  ir: CircuitIR,
  records: readonly CircuitCorrectionRecord[],
): { ir: CircuitIR; records: CircuitCorrectionRecord[] } {
  let current = ir;
  const nextRecords: CircuitCorrectionRecord[] = [];
  for (const record of records) {
    const result = reapplyOne(current, record);
    current = result.ir;
    nextRecords.push(result.record);
  }
  return { ir: current, records: nextRecords };
}

function reapplyOne(
  ir: CircuitIR,
  record: CircuitCorrectionRecord,
): { ir: CircuitIR; record: CircuitCorrectionRecord } {
  if (!targetExists(ir, record.target))
    return { ir, record: { ...record, orphaned: true } };
  try {
    const { ir: next } = applyCircuitCorrections(ir, [record.op], record.author);
    const { orphaned: _orphaned, ...rest } = record;
    return { ir: next, record: rest };
  } catch {
    return { ir, record: { ...record, orphaned: true } };
  }
}

function targetExists(ir: CircuitIR, target: CorrectionTarget): boolean {
  if (target.kind === "element")
    return ir.components.some((component) => component.id === target.id);
  if (target.kind === "net") return ir.nets.some((net) => net.id === target.id);
  // add-block 会新建模块，不要求目标已存在。
  return true;
}
