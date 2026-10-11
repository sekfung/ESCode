import { circuitRuleIdSchema, type CircuitRuleId } from "@escode/shared/circuit";

/**
 * 规则配置（规格 §6.1）：workspace 下 `.escode/circuit-rules.json`，服务只读。
 *
 * 结构与边界：
 * - 整份文件是 `{ "rules": { "<ruleId>": { "enabled"?: boolean, "severity"?: "info" | "warning" | "error" } } }`，
 *   `rules` 可省略，不接受其它字段；任何不符合结构的地方都让整份配置作废（由 app 层报告 status = "invalid"）。
 * - 配置里没有出现的规则用规格 §6 表格的默认级别与启用状态。
 * - 纯函数、零 IO：读取文件由 app 层经 CircuitSourcePort 完成。
 */

export type CircuitRuleSeverity = "info" | "warning" | "error";

export interface CircuitRuleSetting {
  enabled: boolean;
  severity: CircuitRuleSeverity;
}

export type CircuitRuleSettings = Record<CircuitRuleId, CircuitRuleSetting>;

/** 规格 §6 表格的默认级别；表里没有写级别的规则不在这里默认打开。 */
const DEFAULT_SEVERITY: Record<CircuitRuleId, CircuitRuleSeverity> = {
  "unconnected-pin": "warning",
  "single-pin-net": "warning",
  "undriven-power": "error",
  "i2c-missing-pullup": "warning",
  "missing-decoupling": "info",
  "net-name-conflict": "error",
};

const CIRCUIT_RULE_IDS = circuitRuleIdSchema.options;

export function defaultCircuitRuleSettings(): CircuitRuleSettings {
  const settings = {} as CircuitRuleSettings;
  for (const ruleId of CIRCUIT_RULE_IDS)
    settings[ruleId] = { enabled: true, severity: DEFAULT_SEVERITY[ruleId] };
  return settings;
}

export type CircuitRuleConfigParseResult =
  | { ok: true; settings: CircuitRuleSettings }
  | { ok: false; reason: string };

/**
 * 解析配置文件文本。任何结构问题都返回 `ok: false` + 中文原因；
 * 调用方按 §6「配置不合法时整份忽略」处理，并把原因放进 `config.reason`。
 */
export function parseCircuitRuleConfig(text: string): CircuitRuleConfigParseResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text) as unknown;
  } catch (error) {
    return { ok: false, reason: `JSON 解析失败：${error instanceof Error ? error.message : String(error)}` };
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed))
    return { ok: false, reason: "配置根节点必须是对象" };
  const root = parsed as Record<string, unknown>;
  for (const key of Object.keys(root))
    if (key !== "rules") return { ok: false, reason: `不支持的字段 "${key}"（只接受 "rules"）` };
  const settings = defaultCircuitRuleSettings();
  const rules = root.rules;
  if (rules === undefined) return { ok: true, settings };
  if (typeof rules !== "object" || rules === null || Array.isArray(rules))
    return { ok: false, reason: '"rules" 必须是对象' };
  for (const [ruleId, value] of Object.entries(rules as Record<string, unknown>)) {
    if (!isCircuitRuleId(ruleId)) return { ok: false, reason: `未知规则 "${ruleId}"` };
    if (typeof value !== "object" || value === null || Array.isArray(value))
      return { ok: false, reason: `规则 "${ruleId}" 的配置必须是对象` };
    const entry = value as Record<string, unknown>;
    for (const key of Object.keys(entry))
      if (key !== "enabled" && key !== "severity")
        return { ok: false, reason: `规则 "${ruleId}" 有不支持的字段 "${key}"` };
    if (entry.enabled !== undefined && typeof entry.enabled !== "boolean")
      return { ok: false, reason: `规则 "${ruleId}" 的 "enabled" 必须是布尔值` };
    if (entry.severity !== undefined && !isSeverity(entry.severity))
      return { ok: false, reason: `规则 "${ruleId}" 的 "severity" 必须是 info / warning / error` };
    settings[ruleId] = {
      enabled: entry.enabled ?? settings[ruleId].enabled,
      severity: (entry.severity as CircuitRuleSeverity | undefined) ?? settings[ruleId].severity,
    };
  }
  return { ok: true, settings };
}

function isCircuitRuleId(value: string): value is CircuitRuleId {
  return (CIRCUIT_RULE_IDS as readonly string[]).includes(value);
}

function isSeverity(value: unknown): value is CircuitRuleSeverity {
  return value === "info" || value === "warning" || value === "error";
}
