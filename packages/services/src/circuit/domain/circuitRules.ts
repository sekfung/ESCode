import type { CircuitIR, CircuitRuleResult } from "@escode/shared/circuit";
import type { CircuitRuleSeverity } from "./circuitRuleConfig.js";
import {
  isNoConnectPinName,
  isSynthesizedNetName,
  type CircuitRuleFacts,
} from "./circuitRuleFacts.js";

/**
 * 结构类内置规则（规格 §6）：`unconnected-pin`、`single-pin-net`、`net-name-conflict`。
 * 电源/接口启发式在 circuitRulesPower.ts。判定只读 Circuit IR，纯函数、零 IO、结果确定性排序。
 */

/**
 * 引脚是否落在网络之外 / 没有与其它引脚相连。
 * - 没有 netId，或 netId 指向不存在的网络 → 未连接；
 * - 所在网络只有它自己一个引脚、且网络没有名字（解析器的 `Net-(…)` 约定，或识别结果没给名字）→ 未连接；
 * - 单引脚但**具名**的网络（标签 / 电源）说明引脚接进了设计里的某个网络，按 KiCad ERC 的口径不算未连接，
 *   由 `single-pin-net` 报出。
 */
function isDanglingPin(
  facts: CircuitRuleFacts,
  pin: { number: string; name?: string; netId?: string },
): { dangling: boolean; netId?: string } {
  if (pin.netId === undefined) return { dangling: true };
  const net = facts.netById.get(pin.netId);
  if (net === undefined) return { dangling: true };
  if (net.pinRefs.length > 1) return { dangling: false, netId: net.id };
  return { dangling: isSynthesizedNetName(net.name), netId: net.id };
}

/**
 * `unconnected-pin`（warning）：非 NC 引脚未连接。
 * 口径：引脚没有 netId（未挂到任何网络），或所属的**未命名**单引脚网络里只有它自己（判定见 isDanglingPin）。
 * 排除：引脚号为空（机械孔 / NPTH 焊盘）、引脚名形如 NC / N.C. / DNC 的引脚（IR 没有电气类型与
 * no-connect 标记，只能用引脚名代理，见 §6.2）。
 */
export function unconnectedPinRule(
  ir: CircuitIR,
  facts: CircuitRuleFacts,
  severity: CircuitRuleSeverity,
): CircuitRuleResult[] {
  const items: CircuitRuleResult[] = [];
  for (const component of ir.components) {
    for (const pin of component.pins) {
      if (pin.number.trim().length === 0) continue;
      if (isNoConnectPinName(pin.name)) continue;
      const { dangling, netId } = isDanglingPin(facts, pin);
      if (!dangling) continue;
      const label = pin.name ? `${pin.number}（${pin.name}）` : pin.number;
      const message =
        netId === undefined
          ? `器件 ${component.ref} 的引脚 ${label} 没有落在任何网络上`
          : `器件 ${component.ref} 的引脚 ${label} 所在网络 ${netNameOf(facts, netId)} 没有其它引脚`;
      items.push(
        makeItem(
          "unconnected-pin",
          severity,
          message,
          [component.id],
          netId === undefined ? [] : [netId],
          facts,
        ),
      );
    }
  }
  return items;
}

/**
 * `single-pin-net`（warning）：网络只连了一个引脚。
 * 口径：`pinRefs.length === 1`。零引脚网络（dangling-net）与多引脚网络都不算。
 */
export function singlePinNetRule(
  ir: CircuitIR,
  facts: CircuitRuleFacts,
  severity: CircuitRuleSeverity,
): CircuitRuleResult[] {
  const items: CircuitRuleResult[] = [];
  for (const net of ir.nets) {
    if (net.pinRefs.length !== 1) continue;
    const pinRef = net.pinRefs[0]!;
    const component = facts.componentById.get(pinRef.componentId);
    const where = component ? `${component.ref}.${pinRef.pin}` : `${pinRef.componentId}.${pinRef.pin}`;
    items.push(
      makeItem(
        "single-pin-net",
        severity,
        `网络 ${netNameOf(facts, net.id)} 只连了一个引脚（${where}）`,
        component ? [component.id] : [],
        [net.id],
        facts,
      ),
    );
  }
  return items;
}

/**
 * `net-name-conflict`（error）：同一网络有多个名称，或同名网络实际没有连通。
 *
 * 口径：IR 已把同一网络上的多个标签收敛成一个名字（§6.2），因此这里判定后者——
 * 同名但 **id 不同** 的多个网络互为不同网络（例如同一子图纸被实例化两次时各自成网的 local 标签，
 * 见 kicadDesign 的作用域 id 规则）。同一网络有多个名称在 IR v1 里不可见，属声明的 P1 边界。
 */
export function netNameConflictRule(
  ir: CircuitIR,
  facts: CircuitRuleFacts,
  severity: CircuitRuleSeverity,
): CircuitRuleResult[] {
  const items: CircuitRuleResult[] = [];
  for (const [name, nets] of facts.netsByName) {
    if (isSynthesizedNetName(name)) continue;
    const ids = [...new Set(nets.map((net) => net.id))].sort();
    if (ids.length < 2) continue;
    const componentIds = [
      ...new Set(
        nets.flatMap((net) => net.pinRefs.map((pinRef) => pinRef.componentId)),
      ),
    ].sort();
    items.push(
      makeItem(
        "net-name-conflict",
        severity,
        `网络名 ${name} 对应 ${ids.length} 个互不连通的网络`,
        componentIds,
        ids,
        facts,
      ),
    );
  }
  return items;
}

/** 条目涉及的任一元素低置信度 → 标出（§6「涉及低置信度元素的条目要标出来」）。 */
export function makeItem(
  ruleId: CircuitRuleResult["ruleId"],
  severity: CircuitRuleSeverity,
  message: string,
  componentIds: string[],
  netIds: string[],
  facts: CircuitRuleFacts,
): CircuitRuleResult {
  return {
    ruleId,
    severity,
    message,
    componentIds: [...new Set(componentIds)].sort(),
    netIds: [...new Set(netIds)].sort(),
    lowConfidence: [...componentIds, ...netIds].some((id) => facts.isLowConfidenceElement(id)),
  };
}

export function netNameOf(facts: CircuitRuleFacts, netId: string): string {
  const net = facts.netById.get(netId);
  return net?.name ?? netId;
}

/** 同一份 IR 每次得到同样的顺序：ruleId → 首个器件 id → 首个网络 id → 消息。 */
export function sortRuleResults(items: CircuitRuleResult[]): CircuitRuleResult[] {
  return [...items].sort(
    (a, b) =>
      a.ruleId.localeCompare(b.ruleId) ||
      (a.componentIds[0] ?? "").localeCompare(b.componentIds[0] ?? "") ||
      (a.netIds[0] ?? "").localeCompare(b.netIds[0] ?? "") ||
      a.message.localeCompare(b.message),
  );
}
