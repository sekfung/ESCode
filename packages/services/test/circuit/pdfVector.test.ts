import { describe, expect, it } from "vitest";
import { createPdfVectorExtractor } from "../../src/circuit/adapters/pdfVector.js";

/**
 * 矢量 PDF 证据提取（docs/circuit/README.md §2 规则 3、§5 circuit_submit_recognition）。
 * 用例里的 PDF 由本测试现场构造（无压缩流、xref 正确），不依赖 KiCad 或任何样例文件：
 * 文本项与线段的坐标、CTM（`cm` + `q`/`Q`）、以及「没有矢量内容」与「不是 PDF」两种边界都覆盖。
 * pdfjs-dist 的 legacy 构建在 threads worker 里偶发原生崩溃，本文件按 vitest.config.ts 的
 * forkIsolatedTests 隔离到 fork 池运行（与 packages/ui/test/pdfJsCMapSupport.test.ts 同样的处置）。
 */

/** 构造一个 200 × 200 的最小矢量 PDF：`content` 是页面内容流（PDF 用户坐标，原点在左下角）。 */
function minimalPdf(content: string): Uint8Array {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
    `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  let pdf = "%PDF-1.4\n";
  const offsets: number[] = [];
  objects.forEach((body, index) => {
    offsets.push(pdf.length);
    pdf += `${index + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xrefOffset = pdf.length;
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) pdf += `${String(offset).padStart(10, "0")} 00000 n \n`;
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`;
  return new Uint8Array(Buffer.from(pdf, "latin1"));
}

describe("createPdfVectorExtractor", () => {
  it("提取文本项与线段，并换算成左上角原点的页面坐标", async () => {
    const evidence = await createPdfVectorExtractor().extract(
      minimalPdf(
        [
          "1 0 0 RG 2 w",
          "10 10 m 190 10 l S",
          "BT /F1 12 Tf 20 150 Td (R1) Tj ET",
          "q 1 0 0 1 0 50 cm 30 30 m 30 60 l S Q",
        ].join("\n"),
      ),
    );

    expect(evidence?.pageCount).toBe(1);
    // 文本在用户坐标 (20, 150)：y 轴翻转后是 200 − 150 = 50。
    expect(evidence?.texts).toEqual([{ page: 1, text: "R1", x: 20, y: 50 }]);
    // 线段 1 在页面坐标 (10, 10)-(190, 10) → y = 190；线段 2 被 `cm` 平移 50 → (30, 80)-(30, 110)。
    expect(evidence?.segments).toEqual([
      { page: 1, x1: 10, y1: 190, x2: 190, y2: 190 },
      { page: 1, x1: 30, y1: 120, x2: 30, y2: 90 },
    ]);
  });

  it("折线、矩形与曲线：直线段照常提取，曲线不猜形状", async () => {
    const evidence = await createPdfVectorExtractor().extract(
      minimalPdf(
        ["0 200 m 200 200 l 200 0 l S", "20 20 30 40 re f", "5 100 m 6 104 7 108 8 112 c S"].join(
          "\n",
        ),
      ),
    );
    // 折线 2 段 + 矩形 4 段（含 closePath）；曲线只把终点当作当前点，不产生直线段。
    expect(evidence?.segments).toHaveLength(6);
    expect(evidence?.segments).toContainEqual({ page: 1, x1: 0, y1: 0, x2: 200, y2: 0 });
  });

  it("没有文本也没有线段的 PDF（纯位图页面）返回 undefined：调用方按 image 处理", async () => {
    const evidence = await createPdfVectorExtractor().extract(minimalPdf("q 1 0 0 1 0 0 cm Q"));
    expect(evidence).toBeUndefined();
  });

  it("不是 PDF 的字节按 io_error 报错，不静默降级", async () => {
    await expect(
      createPdfVectorExtractor().extract(new Uint8Array(Buffer.from("not a pdf at all", "latin1"))),
    ).rejects.toMatchObject({ code: "io_error" });
  });
});
