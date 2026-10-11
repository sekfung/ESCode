import { describe, expect, it } from "vitest";
import { parseBomCsv } from "../../src/circuit/domain/bom.js";

describe("parseBomCsv", () => {
  it("KiCad 风格逗号 CSV：引号字段、CRLF、未识别列进 fields", () => {
    const csv = [
      '"Id","Refs","Value","Footprint","Datasheet","MPN"',
      '"1","C1, C2","100n","Capacitor_SMD:C_0402_1005Metric","~","CL05B104KO5NNNC"',
      '"2","R1","10k","Resistor_SMD:R_0402_1005Metric","~",""',
    ].join("\r\n");

    const document = parseBomCsv(csv);

    expect(document.rows).toHaveLength(2);
    expect(document.rows[0]).toEqual({
      refs: ["C1", "C2"],
      // 没有 Qty/Quantity 列时不推断数量。
      quantity: undefined,
      value: "100n",
      footprint: "Capacitor_SMD:C_0402_1005Metric",
      partNumber: "CL05B104KO5NNNC",
      fields: { Id: "1", Datasheet: "~" },
    });
    expect(document.rows[1]?.partNumber).toBeUndefined();
    expect(document.rows[1]?.fields).toEqual({ Id: "2", Datasheet: "~" });
  });

  it("Excel 风格分号 CSV：Qty 转数字，列名大小写与空格不敏感", () => {
    const csv = [
      "quantity;REFERENCE;Value;Part Number;Note",
      '2;C1, C2;100n;CL05B104KO5NNNC;"含 ; 分号 与, 逗号"',
      "1;R1;10k;RC0402FR-0710KL;",
    ].join("\n");

    const document = parseBomCsv(csv);

    expect(document.rows).toEqual([
      {
        refs: ["C1", "C2"],
        quantity: 2,
        value: "100n",
        footprint: undefined,
        partNumber: "CL05B104KO5NNNC",
        // 引号内的分号/逗号原样保留。
        fields: { Note: "含 ; 分号 与, 逗号" },
      },
      {
        refs: ["R1"],
        quantity: 1,
        value: "10k",
        footprint: undefined,
        partNumber: "RC0402FR-0710KL",
        fields: { Note: "" },
      },
    ]);
  });

  it("引号字段支持字段内换行与 \"\" 转义", () => {
    const csv = 'Reference,Value,Note\n"C1","100n","第一行\n第二行"\n"R1","10k","说 ""引号"" 处理"\n';

    const document = parseBomCsv(csv);

    expect(document.rows).toHaveLength(2);
    expect(document.rows[0]?.fields).toEqual({ Note: "第一行\n第二行" });
    expect(document.rows[1]?.fields).toEqual({ Note: '说 "引号" 处理' });
  });

  it("refs：逗号/分号/空白都切分，R1-R3 区间原样保留不展开", () => {
    const document = parseBomCsv('Refs,Value\n"C1 C2;R3,R4","misc"\n"R1-R3","10k"\n');

    expect(document.rows[0]?.refs).toEqual(["C1", "C2", "R3", "R4"]);
    // 不做区间展开：Altium 等工具的 R1-R3 在 P1 不解析，原样保留避免误解出多余器件。
    expect(document.rows[1]?.refs).toEqual(["R1-R3"]);
  });

  it("Qty 非数字或为空时省略数量", () => {
    const document = parseBomCsv('Refs,Qty,Value\n"X1","两","x"\n"X2","","y"\n"X3","3","z"\n');

    expect(document.rows.map((row) => row.quantity)).toEqual([undefined, undefined, 3]);
  });

  it("Excel 的 UTF-8 BOM 前缀不影响首列识别，空行跳过，短行按空单元格补齐", () => {
    const csv = '\uFEFFRefs,Qty,Value\r\n"C1",1,"100n"\r\n\r\n"R1","2","10k"\r\n"U1"\r\n';

    const document = parseBomCsv(csv);

    expect(document.rows).toHaveLength(3);
    expect(document.rows[0]?.quantity).toBe(1);
    expect(document.rows[1]).toEqual({
      refs: ["R1"],
      quantity: 2,
      value: "10k",
      footprint: undefined,
      partNumber: undefined,
      fields: {},
    });
    expect(document.rows[2]?.refs).toEqual(["U1"]);
    expect(document.rows[2]?.quantity).toBeUndefined();
    expect(document.rows[2]?.value).toBeUndefined();
  });

  it("没有任何可识别的列名时按无表头处理：返回空 rows", () => {
    expect(parseBomCsv("温度,湿度\n25,60\n")).toEqual({ rows: [] });
    expect(parseBomCsv("")).toEqual({ rows: [] });
  });

  it("传入的不是 BOM（例如 KiCad 文件内容）：不抛错，返回空 rows", () => {
    // CSV 没有“head”可校验，无法识别的输入统一按无表头处理，由上层决定是否换解析器。
    expect(parseBomCsv('(kicad_pcb (version 20240108) (generator "pcbnew")\n  (net 0 "")\n)')).toEqual({
      rows: [],
    });
  });
});
