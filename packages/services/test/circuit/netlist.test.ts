import { describe, expect, it } from "vitest";
import { parseKicadNetlist } from "../../src/circuit/domain/netlist.js";

// 手写最小网表片段，覆盖 KiCad 6（fields/field）与 7/8（property + pinfunction/pintype）两种导出。

const KICAD6_NETLIST = `(export (version "E")
  (design
    (source "/home/user/board/board.kicad_sch")
    (date "2026-10-12 10:00:00")
    (tool "Eeschema 6.0.11")
  )
  (components
    (comp (ref "C1")
      (value "100n")
      (footprint "Capacitor_SMD:C_0402_1005Metric")
      (datasheet "~")
      (fields
        (field (name "Sheetname") "Root")
        (field (name "MPN") "CL05B104KO5NNNC")
      )
    )
    (comp (ref "U1")
      (value "STM32F103C8Tx")
      (footprint "Package_QFP:LQFP-48_7x7mm_P0.5mm")
      (datasheet "https://www.st.com/resource/en/datasheet/stm32f103c8.pdf")
    )
  )
  (libparts
    (libpart (lib "Device") (part "C") (description "Unpolarized capacitor")
      (footprints (fp "C_*"))
    )
  )
  (nets
    (net (code "1") (name "GND")
      (node (ref "C1") (pin "2"))
      (node (ref "U1") (pin "8"))
    )
    (net (code "2") (name "Net-(C1-Pad1)")
      (node (ref "C1") (pin "1"))
    )
  )
)`;

const KICAD8_NETLIST = `(export (version "E")
  (design
    (source "board.kicad_sch")
    (date "2026-10-12")
    (tool "Eeschema 8.0.6")
  )
  (components
    (comp (ref "R1")
      (value "10k")
      (footprint "Resistor_SMD:R_0402_1005Metric")
      (datasheet "~")
      (property "MPN" "RC0402FR-0710KL")
      (property "Description" "厚膜电阻")
    )
  )
  (nets
    (net (code "1") (name "/MCU/BOOT0")
      (node (ref "R1") (pin "1") (pinfunction "~") (pintype "passive"))
    )
  )
)`;

describe("parseKicadNetlist", () => {
  it("KiCad 6 网表：comp 字段与 nets/node 完整解析", () => {
    const document = parseKicadNetlist(KICAD6_NETLIST);

    expect(document.components).toEqual([
      {
        ref: "C1",
        value: "100n",
        footprint: "Capacitor_SMD:C_0402_1005Metric",
        partNumber: "CL05B104KO5NNNC",
      },
      {
        ref: "U1",
        value: "STM32F103C8Tx",
        footprint: "Package_QFP:LQFP-48_7x7mm_P0.5mm",
        partNumber: undefined,
      },
    ]);

    expect(document.nets).toEqual([
      {
        name: "GND",
        code: "1",
        nodes: [
          { ref: "C1", pin: "2" },
          { ref: "U1", pin: "8" },
        ],
      },
      { name: "Net-(C1-Pad1)", code: "2", nodes: [{ ref: "C1", pin: "1" }] },
    ]);
  });

  it("KiCad 7/8 网表：property 提供 MPN，node 的 pinfunction/pintype 不进结果", () => {
    const document = parseKicadNetlist(KICAD8_NETLIST);

    expect(document.components).toEqual([
      {
        ref: "R1",
        value: "10k",
        footprint: "Resistor_SMD:R_0402_1005Metric",
        partNumber: "RC0402FR-0710KL",
      },
    ]);
    // 只保留 ref/pin，pinfunction/pintype 在 IR 的 pinRefs 里没有位置（docs/circuit/README.md §4）。
    expect(document.nets).toEqual([
      { name: "/MCU/BOOT0", code: "1", nodes: [{ ref: "R1", pin: "1" }] },
    ]);
  });

  it("partNumber：property 优先于 fields，字段名大小写不敏感", () => {
    const document = parseKicadNetlist(`(export (version "E")
      (components
        (comp (ref "U2")
          (value "CH340C")
          (property "MPN" "CH340C-来自property")
          (fields (field (name "MPN") "来自fields"))
        )
        (comp (ref "U3")
          (value "AMS1117-3.3")
          (fields (field (name "mpn") "AMS1117-3.3-来自小写字段"))
        )
      )
    )`);

    expect(document.components[0]?.partNumber).toBe("CH340C-来自property");
    expect(document.components[1]?.partNumber).toBe("AMS1117-3.3-来自小写字段");
  });

  it("字段缺失：缺 ref/name/code/pin 时填空值，缺值字段省略", () => {
    const document = parseKicadNetlist(`(export (version "E")
      (components
        (comp (ref "D1"))
        (comp (value "1N4148"))
      )
      (nets
        (net)
        (net (name "N1"))
        (net (code "2") (name "N2") (node (ref "D1")))
      )
    )`);

    expect(document.components).toEqual([
      { ref: "D1", value: undefined, footprint: undefined, partNumber: undefined },
      { ref: "", value: "1N4148", footprint: undefined, partNumber: undefined },
    ]);
    expect(document.nets).toEqual([
      { name: "", code: undefined, nodes: [] },
      { name: "N1", code: undefined, nodes: [] },
      { name: "N2", code: "2", nodes: [{ ref: "D1", pin: "" }] },
    ]);
  });

  it("顶层 head 不是 export：抛 unsupported_format 并带实际 head", () => {
    expect(() => parseKicadNetlist(`(kicad_pcb (version 20240108) (generator "pcbnew"))`)).toThrow(
      /unsupported_format: 顶层 head 为 kicad_pcb/,
    );
  });

  it("不是 s-expression 的文本：抛带说明的 unsupported_format", () => {
    expect(() => parseKicadNetlist("")).toThrow(/unsupported_format.*不是 s-expression 网表/);
  });
});
