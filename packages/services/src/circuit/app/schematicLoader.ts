import { atomAt, firstChild, listHead, parseSExpr } from "../domain/sexpr.js";
import { dirName, joinWorkspacePath } from "../domain/circuitNaming.js";
import { parseKicadSchematic, type KicadSchematicDocument } from "../domain/kicadSchematic.js";
import type { CircuitSourcePort } from "./ports.js";

/**
 * 展开层次化设计需要把根图纸引用的子图纸都读进来（子图纸文件名相对父文件所在目录）。
 * 这里只做「读文件 + 解析」这一层 IO；层级遍历、循环保护与网络合并由 domain/kicadDesign.ts 负责。
 * 限制文件数与深度，避免异常工程（或恶意文件）把解析拖垮。
 */

const MAX_SHEET_FILES = 64;
const MAX_SHEET_DEPTH = 8;

export async function loadSchematicDocuments(params: {
  source: CircuitSourcePort;
  workspacePath: string;
  rootPath: string;
  /** 记录实际读到的文件，解析成功后由服务逐个监听（子图纸变化同样要标 stale）。 */
  sourceFiles: Set<string>;
}): Promise<{ documents: Map<string, KicadSchematicDocument>; rootUuid: string }> {
  const read = async (relativePath: string) => {
    const text = await params.source.readText({
      workspacePath: params.workspacePath,
      relativePath,
    });
    params.sourceFiles.add(relativePath);
    return text;
  };
  const rootText = await read(params.rootPath);
  const documents = new Map<string, KicadSchematicDocument>();
  const rootDocument = parseKicadSchematic(rootText);
  documents.set(params.rootPath, rootDocument);
  const queue: { path: string; document: KicadSchematicDocument; depth: number }[] = [
    { path: params.rootPath, document: rootDocument, depth: 0 },
  ];
  while (queue.length > 0) {
    const current = queue.shift()!;
    if (current.depth >= MAX_SHEET_DEPTH || documents.size >= MAX_SHEET_FILES) continue;
    for (const sheet of current.document.sheets) {
      const childPath = joinWorkspacePath(dirName(current.path), sheet.fileName);
      if (documents.has(childPath) || documents.size >= MAX_SHEET_FILES) continue;
      if (!(await params.source.exists({ workspacePath: params.workspacePath, relativePath: childPath })))
        continue;
      const childDocument = parseKicadSchematic(await read(childPath));
      documents.set(childPath, childDocument);
      queue.push({ path: childPath, document: childDocument, depth: current.depth + 1 });
    }
  }
  return { documents, rootUuid: documentUuid(rootText) ?? "" };
}

/** 根图纸 uuid：KiCad 实例路径的第一段（`/<rootUuid>/<sheetUuid>/…`）。 */
function documentUuid(text: string): string | undefined {
  const root = parseSExpr(text);
  if (listHead(root) !== "kicad_sch") return undefined;
  const node = firstChild(root, "uuid");
  return node ? atomAt(node, 1) : undefined;
}
