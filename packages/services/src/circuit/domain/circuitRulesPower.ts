import type { CircuitComponent, CircuitIR, CircuitNet, CircuitRuleResult } from "@escode/shared/circuit";
import type { CircuitRuleSeverity } from "./circuitRuleConfig.js";
import {
  isCapacitorRef,
  isConnectorRef,
  isGroundNetName,
  isI2cNetName,
  isIcRef,
  isPassiveRef,
  isPowerInputPinName,
  isPowerNetName,
  isResistorRef,
  otherPinNetIds,
  pinOf,
  type CircuitRuleFacts,
} from "./circuitRuleFacts.js";
import { makeItem, netNameOf } from "./circuitRules.js";

/**
 * 电源与接口类内置规则（规格 §6）：`undriven-power`、`i2c-missing-pullup`、`missing-decoupling`。
 *
 * 这三条在规格里就写明是启发式（`missing-decoupling` 标注「启发式」，§7 允许的服务端确定性判定），
 * IR v1 没有电气类型与坐标，判定口径见 CONTRACT.md「规则判定口径」：用引脚名认定电源引脚、
 * 用位号前缀认定器件角色、用网络成员关系代替「附近」。
 */

/**
 * `undriven-power`（error）：电源输入引脚所在网络没有驱动源。
 * 口径：网络上有电源输入引脚（`isPowerInputPinName`），且网上没有任何驱动源——
 * 驱动源 = 连接器/电池类器件（`J` / `P` / `CN` / `USB` / `PS` / `BT` / `BAT`）的任一引脚，
 * 或无源件（`C` / `L` / `FB` / `D` / `R` / …）之外器件的非电源输入引脚（如 LDO 的 `VOUT`）。
 * 只有电源输入引脚 + 无源件的网络判为没有驱动源。
 */
export function undrivenPowerRule(
  ir: CircuitIR,
  facts: CircuitRuleFacts,
  severity: CircuitRuleSeverity,
): CircuitRuleResult[] {
  const items: CircuitRuleResult[] = [];
  for (const net of ir.nets) {
    const powerPins = netPins(net, facts).filter(({ pin }) => isPowerInputPinName(pin.name));
    if (powerPins.length === 0) continue;
    if (hasPowerSource(net, facts)) continue;
    const names = [
      ...new Set(
        powerPins
          .filter(({ pin }) => pin.name !== undefined)
          .map(({ component, pin }) => `${component.ref}.${pin.number}(${pin.name})`),
      ),
    ].sort();
    items.push(
      makeItem(
        "undriven-power",
        severity,
        `网络 ${netNameOf(facts, net.id)} 上有电源输入引脚 ${names.join("、")}，但网上没有驱动源`,
        powerPins.map(({ component }) => component.id),
        [net.id],
        facts,
      ),
    );
  }
  return items;
}

function hasPowerSource(net: CircuitNet, facts: CircuitRuleFacts): boolean {
  return netPins(net, facts).some(({ component, pin }) =>
    isConnectorRef(component.ref)
      ? true
      : !isPassiveRef(component.ref) && !isPowerInputPinName(pin.name),
  );
}

/**
 * `i2c-missing-pullup`（warning）：名称或用途为 I²C 的网络缺少上拉。
 * 口径：网名以 `SDA` / `SCL` 结尾（`isI2cNetName`）即为 I²C 网络；网上必须有一只电阻（`R` / `RN`），
 * 它的另一个引脚连到非地的电源轨网（`isPowerNetName`）。IR 里没有「用途」字段，
 * 只能按网名认定——这是 §6.2 声明的 P1 边界。
 */
export function i2cMissingPullupRule(
  ir: CircuitIR,
  facts: CircuitRuleFacts,
  severity: CircuitRuleSeverity,
): CircuitRuleResult[] {
  const items: CircuitRuleResult[] = [];
  for (const net of ir.nets) {
    if (!isI2cNetName(net.name)) continue;
    if (hasPullup(net, facts)) continue;
    items.push(
      makeItem(
        "i2c-missing-pullup",
        severity,
        `网络 ${netNameOf(facts, net.id)} 是 I²C 网络，但没有电阻上拉到电源轨`,
        net.pinRefs.map((pinRef) => pinRef.componentId),
        [net.id],
        facts,
      ),
    );
  }
  return items;
}

function hasPullup(net: CircuitNet, facts: CircuitRuleFacts): boolean {
  return netPins(net, facts).some(({ component, pin }) => {
    if (!isResistorRef(component.ref)) return false;
    return otherPinNetIds(component, pin, facts).some((netId) =>
      isPowerNetName(netNameOf(facts, netId)),
    );
  });
}

/**
 * `missing-decoupling`（info）：IC 电源引脚附近没有去耦电容（启发式）。
 * 口径：对每个 IC（`U` / `IC` 前缀）的每个电源输入引脚，检查其所在网络；
 * 网上必须有电容（`C` 前缀）且电容的另一个引脚落地网（`isGroundNetName`），否则报出。
 * IR 没有坐标，「附近」用网络成员关系代替：同一电源网上的任一去耦电容都算（§6.2 声明的 P1 边界）。
 */
export function missingDecouplingRule(
  ir: CircuitIR,
  facts: CircuitRuleFacts,
  severity: CircuitRuleSeverity,
): CircuitRuleResult[] {
  const items: CircuitRuleResult[] = [];
  for (const component of ir.components) {
    if (!isIcRef(component.ref)) continue;
    const netIds = [
      ...new Set(
        component.pins
          .filter((pin) => isPowerInputPinName(pin.name) && pin.netId !== undefined)
          .map((pin) => pin.netId!),
      ),
    ].sort();
    for (const netId of netIds) {
      const net = facts.netById.get(netId);
      if (!net || hasDecouplingCapacitor(net, facts)) continue;
      items.push(
        makeItem(
          "missing-decoupling",
          severity,
          `IC ${component.ref} 的电源网络 ${netNameOf(facts, netId)} 上没有接地的去耦电容`,
          [component.id],
          [netId],
          facts,
        ),
      );
    }
  }
  return items;
}

function hasDecouplingCapacitor(net: CircuitNet, facts: CircuitRuleFacts): boolean {
  return netPins(net, facts).some(
    ({ component, pin }) =>
      isCapacitorRef(component.ref) &&
      otherPinNetIds(component, pin, facts).some((netId) =>
        isGroundNetName(netNameOf(facts, netId)),
      ),
  );
}

/** 网络上的引脚及其器件；IR 校验已保证 pinRefs 指向真实元素，找不到的引用在这里跳过。 */
function netPins(
  net: CircuitNet,
  facts: CircuitRuleFacts,
): { component: CircuitComponent; pin: { number: string; name?: string; netId?: string } }[] {
  const pins: {
    component: CircuitComponent;
    pin: { number: string; name?: string; netId?: string };
  }[] = [];
  for (const pinRef of net.pinRefs) {
    const component = facts.componentById.get(pinRef.componentId);
    if (!component) continue;
    const pin = pinOf(component, pinRef.pin);
    if (!pin) continue;
    pins.push({ component, pin });
  }
  return pins;
}
