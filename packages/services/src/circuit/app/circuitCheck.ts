import {
  CIRCUIT_RULE_CONFIG_PATH,
  type CircuitIR,
  type CircuitRuleConfigStatus,
} from "@escode/shared/circuit";
import { createServiceLogger } from "../../logger/serviceLogger.js";
import type { CircuitCheckParams, CircuitCheckResult } from "../contract.js";
import {
  defaultCircuitRuleSettings,
  parseCircuitRuleConfig,
  type CircuitRuleSettings,
} from "../domain/circuitRuleConfig.js";
import { runCircuitRules } from "../domain/circuitRuleChecks.js";
import { paginateAnalysisResult } from "./analysisPagination.js";
import type { CircuitSourcePort } from "./ports.js";

const log = createServiceLogger("circuit");

/**
 * `circuit_check` 的编排（规格 §6）：确认 IR → 读配置 → 跑规则 → 分页。
 * 只读、不改 IR；配置读取失败只影响 `config` 字段，检查本身照常返回（§6.1）。
 */
export async function runCircuitCheck(
  params: CircuitCheckParams,
  source: CircuitSourcePort,
  loadIr: () => Promise<CircuitIR>,
): Promise<CircuitCheckResult> {
  const ir = await loadIr();
  const { status, settings } = await loadCircuitRuleSettings({
    source,
    workspacePath: params.workspacePath,
  });
  const all = runCircuitRules(ir, settings);
  const signature = JSON.stringify(["check", params.irId]);
  const { items, nextCursor } = paginateAnalysisResult(all, params.cursor, signature);
  if (status.status === "invalid")
    log.warn("circuit rule config invalid", {
      irId: params.irId,
      path: status.path,
      reason: status.reason,
    });
  log.info("circuit check", {
    irId: params.irId,
    config: status.status,
    items: all.length,
    returned: items.length,
  });
  return { config: status, items, ...(nextCursor ? { nextCursor } : {}) };
}

/**
 * 规则配置读取（规格 §6.1）：workspace 下 `.escode/circuit-rules.json`，**只读**。
 * - 没有文件 → `status: "default"`，用默认配置；
 * - 文件存在且合法 → `status: "applied"`；
 * - 文件存在但解析失败 / 结构不符 / 读不出来 → 整份忽略，`status: "invalid"` + `reason`，按默认配置检查。
 * 配置是可选文件，读取失败不把规则检查整体变成错误，但必须在结果里可见（§6）。
 */
export async function loadCircuitRuleSettings(params: {
  source: CircuitSourcePort;
  workspacePath: string;
}): Promise<{ status: CircuitRuleConfigStatus; settings: CircuitRuleSettings }> {
  const settings = defaultCircuitRuleSettings();
  let exists: boolean;
  try {
    exists = await params.source.exists({
      workspacePath: params.workspacePath,
      relativePath: CIRCUIT_RULE_CONFIG_PATH,
    });
  } catch (error) {
    return {
      status: {
        status: "invalid",
        path: CIRCUIT_RULE_CONFIG_PATH,
        reason: `读取规则配置失败：${messageOf(error)}`,
      },
      settings,
    };
  }
  if (!exists) return { status: { status: "default" }, settings };
  let text: string;
  try {
    text = await params.source.readText({
      workspacePath: params.workspacePath,
      relativePath: CIRCUIT_RULE_CONFIG_PATH,
    });
  } catch (error) {
    return {
      status: {
        status: "invalid",
        path: CIRCUIT_RULE_CONFIG_PATH,
        reason: `读取规则配置失败：${messageOf(error)}`,
      },
      settings,
    };
  }
  const parsed = parseCircuitRuleConfig(text);
  if (!parsed.ok)
    return {
      status: { status: "invalid", path: CIRCUIT_RULE_CONFIG_PATH, reason: parsed.reason },
      settings,
    };
  return { status: { status: "applied", path: CIRCUIT_RULE_CONFIG_PATH }, settings: parsed.settings };
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
