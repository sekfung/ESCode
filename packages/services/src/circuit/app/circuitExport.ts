import { CIRCUIT_EXPORT_SUFFIX } from "@escode/shared/circuit";
import type { CircuitExportParams, CircuitExportResult } from "../contract.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";
import type { CircuitLocation } from "./circuitLocation.js";
import type { CircuitSnapshotPort, StoredCircuit } from "./ports.js";

/**
 * 导出只读快照的内容与写入（§2 规则 8、§5）：写入 workspace 的派生产物，内容带 derived 标记。
 * 「服务永远不读回它」由调用方保证：导出路径不进 store、也不注册源文件监听。
 * 目标扩展名由服务端约束为 `.circuit.json`（§8 的导出快照；shared CONTRACT.md 待定项）。
 */
export async function writeCircuitSnapshot(input: {
  location: CircuitLocation;
  record: StoredCircuit;
  params: CircuitExportParams;
  snapshot: CircuitSnapshotPort;
  now: () => string;
}): Promise<CircuitExportResult> {
  if (!input.params.path.trim().endsWith(CIRCUIT_EXPORT_SUFFIX))
    throw new CircuitServiceError(
      "unsupported_format",
      `导出快照的目标文件必须以 ${CIRCUIT_EXPORT_SUFFIX} 结尾：${input.params.path}`,
    );
  const content = JSON.stringify(
    {
      derived: true,
      generatedBy: "circuit-service",
      irId: input.record.irId,
      revision: input.record.ir.revision,
      exportedAt: input.now(),
      ir: input.record.ir,
    },
    null,
    2,
  );
  await input.snapshot.write({
    workspacePath: input.location.workspacePath,
    relativePath: input.params.path,
    content: `${content}\n`,
  });
  return { path: input.params.path };
}
