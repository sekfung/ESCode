/**
 * pdfjs-dist 的 worker 子路径没有随包发布类型（只有 `legacy/build/pdf.d.mts`）。
 * circuit 的矢量 PDF 提取按 pdfjs 支持的接法把 worker 模块挂到 `globalThis.pdfjsWorker`（见
 * circuit/adapters/pdfVector.ts），这里只声明用到的那个导出。
 */
declare module "pdfjs-dist/legacy/build/pdf.worker.mjs" {
  export const WorkerMessageHandler: unknown;
}
