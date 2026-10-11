import type { CircuitSourceKind } from "@escode/shared/circuit";
import { CircuitServiceError } from "../domain/circuitErrors.js";
import { buildKicadDesign } from "../domain/kicadDesign.js";
import { buildKicadPcbLayout, buildKicadSchematicLayout } from "../domain/kicadLayout.js";
import { parseBomCsv } from "../domain/bom.js";
import { parseKicadPcb } from "../domain/kicadPcb.js";
import { parseKicadNetlist } from "../domain/netlist.js";
import {
  elementsFromBom,
  elementsFromNetlist,
  elementsFromPcb,
  elementsFromSchematicDesign,
  type ParsedCircuitElements,
} from "../domain/irBuild.js";
import type { CircuitSourcePort, StoredCircuit } from "./ports.js";
import { loadSchematicDocuments } from "./schematicLoader.js";

/** 确定性解析的来源分派（§3.1）：按扩展名决定用哪个解析器，产出 IR 元素与渲染几何。 */

export function detectCircuitSourceKind(path: string): CircuitSourceKind {
  const lower = path.toLowerCase();
  if (lower.endsWith(".kicad_sch")) return "kicad_sch";
  if (lower.endsWith(".kicad_pcb")) return "kicad_pcb";
  if (lower.endsWith(".net") || lower.endsWith(".netlist")) return "netlist";
  if (lower.endsWith(".csv")) return "bom";
  throw new CircuitServiceError(
    "unsupported_format",
    `只支持 .kicad_sch / .kicad_pcb / .net / .csv（收到 ${path}）`,
  );
}

export async function extractCircuitSource(params: {
  kind: CircuitSourceKind;
  text: string;
  workspacePath: string;
  sourcePath: string;
  irId: string;
  revision: number;
  source: CircuitSourcePort;
  /** 记录这次解析实际读到的文件（含子图纸），解析成功后逐个监听。 */
  sourceFiles: Set<string>;
}): Promise<{ elements: ParsedCircuitElements; layout?: StoredCircuit["layout"] }> {
  switch (params.kind) {
    case "kicad_sch": {
      const loaded = await loadSchematicDocuments({
        source: params.source,
        workspacePath: params.workspacePath,
        rootPath: params.sourcePath,
        sourceFiles: params.sourceFiles,
      });
      const design = buildKicadDesign({
        rootPath: params.sourcePath,
        rootUuid: loaded.rootUuid,
        documents: loaded.documents,
      });
      return {
        elements: elementsFromSchematicDesign(design),
        layout: buildKicadSchematicLayout({
          rootPath: params.sourcePath,
          rootUuid: loaded.rootUuid,
          documents: loaded.documents,
          irId: params.irId,
          revision: params.revision,
        }),
      };
    }
    case "kicad_pcb": {
      const pcb = parseKicadPcb(params.text);
      const elements = elementsFromPcb(pcb);
      return {
        elements,
        layout: buildKicadPcbLayout({
          pcb,
          irId: params.irId,
          revision: params.revision,
          components: elements.components.map((component) => ({
            id: component.id,
            ref: component.ref,
          })),
        }),
      };
    }
    case "netlist":
      return { elements: elementsFromNetlist(parseKicadNetlist(params.text)) };
    case "bom":
      return { elements: elementsFromBom(parseBomCsv(params.text)) };
    default:
      throw new CircuitServiceError("unsupported_format", `不能确定性解析 ${params.kind}`);
  }
}
