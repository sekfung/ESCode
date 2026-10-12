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

/** 识别来源的扩展名（§5 circuit_submit_recognition 只接受 PDF 与图片）。 */
const RECOGNITION_IMAGE_EXTENSIONS = [".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"] as const;

/**
 * 识别来源分派（§5）：`.pdf` 走矢量提取（页内只有位图时按 image 处理），
 * 常见图片扩展名按 image；其余（例如 `.kicad_sch`）报 `unsupported_format`，确定性解析走 `circuit_parse`。
 */
export function detectRecognitionSourceKind(path: string): "pdf_vector" | "image" {
  const lower = path.toLowerCase();
  if (lower.endsWith(".pdf")) return "pdf_vector";
  if (RECOGNITION_IMAGE_EXTENSIONS.some((extension) => lower.endsWith(extension))) return "image";
  throw new CircuitServiceError(
    "unsupported_format",
    `circuit_submit_recognition 只支持 PDF 与图片（${RECOGNITION_IMAGE_EXTENSIONS.join(" / ")}）：${path}`,
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
