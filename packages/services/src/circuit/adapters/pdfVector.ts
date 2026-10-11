import { createServiceLogger } from "../../logger/serviceLogger.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";
import type {
  CircuitVectorEvidence,
  CircuitVectorSegment,
  CircuitVectorText,
} from "../domain/recognitionEvidence.js";
import type { CircuitVectorPort } from "../app/ports.js";
import type { PageViewport, PDFDocumentProxy } from "pdfjs-dist/legacy/build/pdf.mjs";

/**
 * 矢量 PDF 的确定性证据提取（docs/circuit/README.md §2 规则 3、§5 circuit_submit_recognition）：
 * 用仓库已有的 pdfjs-dist 读出文本项与线段，坐标统一换成**左上角为原点**的页面坐标（1:1 缩放），
 * 与草稿的 `region`、面板叠框的坐标口径一致（见 domain/recognitionEvidence.ts）。
 * 只做数据提取、不做渲染：不需要 canvas，也不加载字体数据。
 *
 * pdfjs 按需加载：Desktop 的 Host bundle 会内联 @escode/services，内联后 pdfjs 默认的
 * 「假 worker」路径（按 `workerSrc` 动态 import 兄弟文件 `pdf.worker.mjs`）解析不到文件而报
 * `Setting up fake worker failed`。这里按 pdfjs 支持的接法把 worker 模块挂到 `globalThis.pdfjsWorker`，
 * 它就直接用这个模块、不再解析路径；同时避免让 Host 启动时为这个低频能力付 pdfjs 的加载开销。
 */

const log = createServiceLogger("circuit");

type PdfJsModule = typeof import("pdfjs-dist/legacy/build/pdf.mjs");

let pdfjsPromise: Promise<PdfJsModule> | undefined;

function loadPdfjs(): Promise<PdfJsModule> {
  pdfjsPromise ??= Promise.all([
    import("pdfjs-dist/legacy/build/pdf.mjs"),
    import("pdfjs-dist/legacy/build/pdf.worker.mjs"),
  ]).then(([pdfjs, worker]) => {
    (globalThis as { pdfjsWorker?: unknown }).pdfjsWorker ??= worker;
    return pdfjs;
  });
  return pdfjsPromise;
}

/** PDF 矩阵 [a, b, c, d, e, f]（行向量约定：x' = ax + cy + e，y' = bx + dy + f）。 */
type Matrix = readonly [number, number, number, number, number, number];

const IDENTITY: Matrix = [1, 0, 0, 1, 0, 0];

/** `m` 先作用、`n` 后作用（对应 PDF 的 `cm`：CTM' = M × CTM）。 */
function multiply(m: Matrix, n: Matrix): Matrix {
  return [
    m[0] * n[0] + m[1] * n[2],
    m[0] * n[1] + m[1] * n[3],
    m[2] * n[0] + m[3] * n[2],
    m[2] * n[1] + m[3] * n[3],
    m[4] * n[0] + m[5] * n[2] + n[4],
    m[4] * n[1] + m[5] * n[3] + n[5],
  ];
}

/**
 * `OPS.constructPath` 的路径数据是按「(子操作码, x, y[, …])」铺平的浮点数组：
 * 0 = moveTo，1 = lineTo，2 = curveTo（后跟两个控制点与终点），3 = closePath。
 * 只提取直线段（KiCad 的导线就是直线）：曲线不猜形状，直接跳过该段。
 */
const PATH_MOVE = 0;
const PATH_LINE = 1;
const PATH_CURVE = 2;
const PATH_CLOSE = 3;

interface Point {
  x: number;
  y: number;
}

function collectSegments(
  pathData: Float32Array,
): { x1: number; y1: number; x2: number; y2: number }[] {
  const segments: { x1: number; y1: number; x2: number; y2: number }[] = [];
  let current: Point | undefined;
  let start: Point | undefined;
  let index = 0;
  while (index < pathData.length) {
    const op = pathData[index];
    if (op === PATH_CURVE) {
      // curveTo：op + 两个控制点 + 终点，只把终点当作当前点（不产生直线段）。
      const end = { x: pathData[index + 5]!, y: pathData[index + 6]! };
      current = end;
      index += 7;
      continue;
    }
    if (op === PATH_MOVE) {
      current = { x: pathData[index + 1]!, y: pathData[index + 2]! };
      start = current;
      index += 3;
      continue;
    }
    if (op === PATH_LINE) {
      const next = { x: pathData[index + 1]!, y: pathData[index + 2]! };
      if (current) segments.push({ x1: current.x, y1: current.y, x2: next.x, y2: next.y });
      current = next;
      index += 3;
      continue;
    }
    if (op === PATH_CLOSE) {
      if (current && start)
        segments.push({ x1: current.x, y1: current.y, x2: start.x, y2: start.y });
      current = start;
      index += 1;
      continue;
    }
    // 不认识的子操作码：整条路径到此为止，不猜测其余数据的布局。
    break;
  }
  return segments;
}

/** 路径坐标经 CTM 与视口变换换成左上角原点的页面坐标。 */
function toViewportPoint(viewport: PageViewport, ctm: Matrix, point: Point) {
  const [x, y] = viewport.convertToViewportPoint(
    ctm[0] * point.x + ctm[2] * point.y + ctm[4],
    ctm[1] * point.x + ctm[3] * point.y + ctm[5],
  );
  return { x, y };
}

export function createPdfVectorExtractor(): CircuitVectorPort {
  return {
    async extract(bytes: Uint8Array): Promise<CircuitVectorEvidence | undefined> {
      const pdfjs = await loadPdfjs();
      let document: PDFDocumentProxy;
      try {
        document = await pdfjs.getDocument({
          data: bytes,
          // 只取文本与路径，不做渲染：不需要字体数据、不执行 PDF 里的脚本。
          disableFontFace: true,
          isEvalSupported: false,
          useSystemFonts: false,
          verbosity: 0,
        }).promise;
      } catch (error) {
        throw new CircuitServiceError(
          "io_error",
          `PDF 解析失败：${error instanceof Error ? error.message : String(error)}`,
        );
      }
      const texts: CircuitVectorText[] = [];
      const segments: CircuitVectorSegment[] = [];
      try {
        for (let pageNumber = 1; pageNumber <= document.numPages; pageNumber += 1) {
          const page = await document.getPage(pageNumber);
          const viewport = page.getViewport({ scale: 1 });
          const content = await page.getTextContent();
          for (const item of content.items)
            if ("str" in item && item.str.trim().length > 0) {
              const [x, y] = viewport.convertToViewportPoint(item.transform[4], item.transform[5]);
              texts.push({ page: pageNumber, text: item.str.trim(), x, y });
            }
          const operatorList = await page.getOperatorList();
          // CTM 栈：save / restore / transform 与画布语义一致，解码构造路径里的原始坐标。
          const stack: Matrix[] = [];
          let ctm: Matrix = IDENTITY;
          for (let index = 0; index < operatorList.fnArray.length; index += 1) {
            const op = operatorList.fnArray[index];
            const args = operatorList.argsArray[index];
            if (op === pdfjs.OPS.save) stack.push(ctm);
            else if (op === pdfjs.OPS.restore) ctm = stack.pop() ?? IDENTITY;
            else if (op === pdfjs.OPS.transform) {
              const [a, b, c, d, e, f] = args as number[];
              ctm = multiply([a!, b!, c!, d!, e!, f!], ctm);
            } else if (op === pdfjs.OPS.constructPath) {
              const pathData = pathDataOf(args);
              if (!pathData) continue;
              for (const segment of collectSegments(pathData)) {
                const from = toViewportPoint(viewport, ctm, { x: segment.x1, y: segment.y1 });
                const to = toViewportPoint(viewport, ctm, { x: segment.x2, y: segment.y2 });
                segments.push({ page: pageNumber, x1: from.x, y1: from.y, x2: to.x, y2: to.y });
              }
            }
          }
        }
      } catch (error) {
        throw new CircuitServiceError(
          "io_error",
          `PDF 内容读取失败：${error instanceof Error ? error.message : String(error)}`,
        );
      }
      if (texts.length === 0 && segments.length === 0) {
        log.info("circuit pdf has no vector content", { pages: document.numPages });
        return undefined;
      }
      log.debug("circuit pdf vector extracted", {
        pages: document.numPages,
        texts: texts.length,
        segments: segments.length,
      });
      return { pageCount: document.numPages, texts, segments };
    },
  };
}

/**
 * pdfjs 5.x 的 constructPath 参数是 `[绘制操作码, [铺平的路径数据], minMax]`，
 * 路径数据是 DrawOPS 码与坐标铺平的 Float32Array（pdfjs 自己同样只取 `data[0]`）。
 * 已经渲染过的路径会被换成 Path2D，这里认不出来就跳过该条路径，不猜数据布局。
 */
function pathDataOf(args: unknown): Float32Array | undefined {
  const data = (args as unknown[])[1];
  const [path] = Array.isArray(data) ? data : [];
  return path instanceof Float32Array ? path : undefined;
}
