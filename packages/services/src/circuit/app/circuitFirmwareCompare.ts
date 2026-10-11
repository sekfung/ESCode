import type { CircuitIR } from "@escode/shared/circuit";
import { createServiceLogger } from "../../logger/serviceLogger.js";
import type { CircuitFirmwareCompareParams, CircuitFirmwareCompareResult } from "../contract.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";
import { compareFirmwarePins } from "../domain/firmwareCompare.js";
import { assertFirmwareFormat, parseFirmwarePins } from "../domain/firmwareParse.js";
import type { FirmwarePinDeclaration } from "../domain/firmwarePins.js";
import { paginateAnalysisResult } from "./analysisPagination.js";
import type { CircuitSourcePort } from "./ports.js";

const log = createServiceLogger("circuit");

/**
 * `circuit_compare_firmware` 的编排（规格 §7）：确认 IR → 读固件文件 → 解析 → 比对 → 分页。
 * 空文件列表报 `invalid_ir`（读操作不写状态、不需要 baseRevision）。
 */
export async function runCircuitFirmwareCompare(
  params: CircuitFirmwareCompareParams,
  source: CircuitSourcePort,
  loadIr: () => Promise<CircuitIR>,
): Promise<CircuitFirmwareCompareResult> {
  if (params.files.length === 0)
    throw new CircuitServiceError("invalid_ir", "固件比对至少需要一个文件");
  const ir = await loadIr();
  const declarations = await loadFirmwareDeclarations({
    source,
    workspacePath: params.workspacePath,
    files: params.files,
  });
  const all = compareFirmwarePins(ir, declarations);
  const signature = JSON.stringify(["compare-firmware", params.irId, params.files]);
  const { items, nextCursor } = paginateAnalysisResult(all, params.cursor, signature);
  log.info("circuit compare firmware", {
    irId: params.irId,
    files: params.files.length,
    declarations: declarations.length,
    differences: all.length,
    returned: items.length,
  });
  return { items, ...(nextCursor ? { nextCursor } : {}) };
}

/**
 * 固件文件读取与解析（规格 §7）：逐个按 workspace 相对路径读文本（只读、异步），
 * 按扩展名分派到 devicetree / `.ioc` / C 头文件解析器；解析结果拼接后交给比对。
 * 先判扩展名再读盘：不支持的格式直接报 `unsupported_format`，不必等 IO。
 * 文件不存在或读取失败由 CircuitSourcePort 抛出对应错误码（`not_found` / `too_large` / `io_error`）。
 */
export async function loadFirmwareDeclarations(params: {
  source: CircuitSourcePort;
  workspacePath: string;
  files: string[];
}): Promise<FirmwarePinDeclaration[]> {
  const declarations: FirmwarePinDeclaration[] = [];
  for (const file of params.files) {
    assertFirmwareFormat(file);
    const text = await params.source.readText({
      workspacePath: params.workspacePath,
      relativePath: file,
    });
    declarations.push(...parseFirmwarePins({ path: file, text }));
  }
  return declarations;
}
