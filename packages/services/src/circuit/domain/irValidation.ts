import type { CircuitDiagnostic, CircuitIR } from "@escode/shared/circuit";

/**
 * IR 一致性校验（§4）：识别结果与解析结果都用同一套判定。
 * 校验不通过的元素只写 diagnostics，不在这里降置信度——降置信度是提交识别时的服务端行为。
 * 输出按 (code, 元素 id) 排序，保证同一份 IR 每次校验得到同样的顺序。
 */
export function validateCircuitIr(ir: CircuitIR): CircuitDiagnostic[] {
  const diagnostics: CircuitDiagnostic[] = [];
  const componentById = new Map(ir.components.map((component) => [component.id, component]));
  const netById = new Map(ir.nets.map((net) => [net.id, net]));

  const idsByRef = new Map<string, string[]>();
  for (const component of ir.components) {
    const list = idsByRef.get(component.ref);
    if (list) list.push(component.id);
    else idsByRef.set(component.ref, [component.id]);
  }
  for (const [ref, ids] of [...idsByRef].sort(([a], [b]) => a.localeCompare(b))) {
    if (ids.length < 2) continue;
    diagnostics.push({
      code: "duplicate-ref",
      severity: "error",
      message: `位号 ${ref} 出现 ${ids.length} 次`,
      componentIds: [...ids].sort(),
    });
  }

  for (const component of ir.components) {
    for (const pin of component.pins) {
      if (!pin.netId) continue;
      const net = netById.get(pin.netId);
      if (!net) {
        diagnostics.push({
          code: "unknown-net-reference",
          severity: "error",
          message: `器件 ${component.ref} 的引脚 ${pin.number} 指向不存在的网络 ${pin.netId}`,
          componentIds: [component.id],
        });
        continue;
      }
      if (
        !net.pinRefs.some(
          (pinRef) => pinRef.componentId === component.id && pinRef.pin === pin.number,
        )
      )
        diagnostics.push({
          code: "unknown-net-reference",
          severity: "error",
          message: `网络 ${net.id} 没有包含器件 ${component.ref} 的引脚 ${pin.number}`,
          componentIds: [component.id],
          netIds: [net.id],
        });
    }
  }

  for (const net of ir.nets) {
    const sorted = [...net.pinRefs].sort(
      (a, b) => a.componentId.localeCompare(b.componentId) || a.pin.localeCompare(b.pin),
    );
    for (const pinRef of sorted) {
      const component = componentById.get(pinRef.componentId);
      if (!component) {
        diagnostics.push({
          code: "unknown-component-reference",
          severity: "error",
          message: `网络 ${net.name ?? net.id} 引用了不存在的器件 ${pinRef.componentId}`,
          netIds: [net.id],
        });
        continue;
      }
      if (!component.pins.some((pin) => pin.number === pinRef.pin))
        diagnostics.push({
          code: "unknown-component-reference",
          severity: "error",
          message: `网络 ${net.name ?? net.id} 引用了器件 ${component.ref} 上不存在的引脚 ${pinRef.pin}`,
          componentIds: [component.id],
          netIds: [net.id],
        });
    }
    if (sorted.length === 0)
      diagnostics.push({
        code: "dangling-net",
        severity: "warning",
        message: `网络 ${net.name ?? net.id} 没有连接任何引脚`,
        netIds: [net.id],
      });
    else if (sorted.length === 1)
      diagnostics.push({
        code: "single-pin-net",
        severity: "warning",
        message: `网络 ${net.name ?? net.id} 只连了一个引脚`,
        netIds: [net.id],
      });
  }

  for (const block of ir.blocks) {
    for (const componentId of [...block.componentIds].sort()) {
      if (componentById.has(componentId)) continue;
      diagnostics.push({
        code: "unknown-component-reference",
        severity: "error",
        message: `模块 ${block.name} 引用了不存在的器件 ${componentId}`,
        componentIds: [componentId],
      });
    }
  }

  return diagnostics.sort(
    (a, b) =>
      a.code.localeCompare(b.code) ||
      (a.componentIds?.[0] ?? "").localeCompare(b.componentIds?.[0] ?? "") ||
      (a.netIds?.[0] ?? "").localeCompare(b.netIds?.[0] ?? "") ||
      a.message.localeCompare(b.message),
  );
}
