import type { CircuitIR, CircuitRuleResult } from "@escode/shared/circuit";
import type { CircuitRuleSettings } from "./circuitRuleConfig.js";
import { buildCircuitRuleFacts, type CircuitRuleFacts } from "./circuitRuleFacts.js";
import {
  netNameConflictRule,
  singlePinNetRule,
  sortRuleResults,
  unconnectedPinRule,
} from "./circuitRules.js";
import {
  i2cMissingPullupRule,
  missingDecouplingRule,
  undrivenPowerRule,
} from "./circuitRulesPower.js";

/**
 * 内置规则检查入口（规格 §6）：按配置逐条执行，停用的规则不产生条目，级别用配置覆盖值。
 * 纯函数：同一份 (IR, 配置) 每次得到同样的排序结果。
 */
export function runCircuitRules(ir: CircuitIR, settings: CircuitRuleSettings): CircuitRuleResult[] {
  const facts = buildCircuitRuleFacts(ir);
  const items: CircuitRuleResult[] = [];
  const run = (ruleId: keyof CircuitRuleSettings, rule: Rule): void => {
    const setting = settings[ruleId];
    if (setting.enabled) items.push(...rule(ir, facts, setting.severity));
  };
  run("unconnected-pin", unconnectedPinRule);
  run("single-pin-net", singlePinNetRule);
  run("undriven-power", undrivenPowerRule);
  run("i2c-missing-pullup", i2cMissingPullupRule);
  run("missing-decoupling", missingDecouplingRule);
  run("net-name-conflict", netNameConflictRule);
  return sortRuleResults(items);
}

type Rule = (
  ir: CircuitIR,
  facts: CircuitRuleFacts,
  severity: "info" | "warning" | "error",
) => CircuitRuleResult[];
