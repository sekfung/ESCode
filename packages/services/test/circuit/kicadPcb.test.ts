import { describe, expect, it } from "vitest";
import { parseKicadPcb } from "../../src/circuit/domain/kicadPcb.js";

// 手写最小片段，覆盖 KiCad 6 与 7/8 两种写法；不依赖正在下载的真实板子 fixture。

const KICAD8_BOARD = `(kicad_pcb (version 20240108) (generator "pcbnew") (generator_version "8.0")
  (general
    (thickness 1.6)
  )
  (paper "A4")
  (layers
    (0 "F.Cu" signal)
    (31 "B.Cu" signal)
    (44 "Edge.Cuts" user)
  )
  (net 0 "")
  (net 1 "GND")
  (net 2 "+3V3")
  (gr_rect (start 0 0) (end 60 40.5)
    (stroke (width 0.05) (type default))
    (fill none)
    (layer "Edge.Cuts")
    (uuid "aaaaaaaa-0000-0000-0000-000000000001")
  )
  (gr_line (start -10 -10) (end 100 100)
    (stroke (width 0.12) (type default))
    (layer "F.SilkS")
    (uuid "bbbbbbbb-0000-0000-0000-000000000002")
  )
  (footprint "Resistor_SMD:R_0402_1005Metric" (layer "F.Cu")
    (uuid "11111111-1111-1111-1111-111111111111")
    (at 25 20 90)
    (property "Reference" "R1" (at 0 0 90) (layer "F.SilkS")
      (uuid "11111111-1111-1111-1111-111111111112")
      (effects (font (size 1 1) (thickness 0.15)))
    )
    (property "Value" "10k" (at 0 0 90) (layer "F.Fab")
      (uuid "11111111-1111-1111-1111-111111111113")
      (effects (font (size 1 1) (thickness 0.15)))
    )
    (property "Footprint" "Resistor_SMD:R_0402_1005Metric" (at 0 0 0) (layer "F.Fab")
      (uuid "11111111-1111-1111-1111-111111111114")
      (attr smd)
    )
    (pad "1" smd roundrect (at -0.51 0 90) (size 0.6 0.62) (layers "F.Cu" "F.Paste" "F.Mask")
      (roundrect_rratio 0.25)
      (net 2 "+3V3")
      (uuid "11111111-1111-1111-1111-111111111115")
    )
    (pad "2" smd roundrect (at 0.51 0 90) (size 0.6 0.62) (layers "F.Cu" "F.Paste" "F.Mask")
      (roundrect_rratio 0.25)
      (net 1 "GND")
      (uuid "11111111-1111-1111-1111-111111111116")
    )
  )
)`;

const KICAD6_BOARD = `(kicad_pcb (version 20211014) (generator pcbnew)
  (net 0 "")
  (net 1 GND)
  (gr_line (start 0 0) (end 50 0) (layer "Edge.Cuts") (width 0.05) (tstamp 00000000-0000-0000-0000-0000000000a1))
  (gr_line (start 50 0) (end 50 30) (layer "Edge.Cuts") (width 0.05) (tstamp 00000000-0000-0000-0000-0000000000a2))
  (gr_circle (center 90 20) (end 86 20) (layer "Edge.Cuts") (width 0.05) (tstamp 00000000-0000-0000-0000-0000000000a3))
  (footprint "Package_TO_SOT_SMD:SOT-23" (layer "F.Cu") (tstamp 22222222-2222-2222-2222-222222222222)
    (at 10 5)
    (fp_text reference "Q1" (at 0 -2.5) (layer "F.SilkS") (tstamp 00000000-0000-0000-0000-0000000000b1)
      (effects (font (size 1 1) (thickness 0.15)))
    )
    (fp_text value "2N7002" (at 0 2.5) (layer "F.Fab") (tstamp 00000000-0000-0000-0000-0000000000b2)
      (effects (font (size 1 1) (thickness 0.15)))
    )
    (fp_text user "\${REFERENCE}" (at 0 0) (layer "F.Fab") (tstamp 00000000-0000-0000-0000-0000000000b3)
      (effects (font (size 1 1) (thickness 0.15)))
    )
    (pad "1" smd rect (at -1 -0.95) (size 1.2 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (net 1 GND) (tstamp 00000000-0000-0000-0000-0000000000c1))
    (pad "2" smd rect (at 1 -0.95) (size 1.2 1.2) (layers "F.Cu" "F.Paste" "F.Mask") (tstamp 00000000-0000-0000-0000-0000000000c2))
  )
)`;

describe("parseKicadPcb", () => {
  it("KiCad 8 板：解析板框、网络、封装属性与焊盘连接", () => {
    const document = parseKicadPcb(KICAD8_BOARD);

    // 板框只看 Edge.Cuts；F.SilkS 上那条 (-10,-10)-(100,100) 的线不参与。
    expect(document.outline).toEqual({ width: 60, height: 40.5 });
    expect(document.nets).toEqual([
      { number: 0, name: "" },
      { number: 1, name: "GND" },
      { number: 2, name: "+3V3" },
    ]);

    expect(document.footprints).toHaveLength(1);
    const footprint = document.footprints[0]!;
    expect(footprint.id).toBe("11111111-1111-1111-1111-111111111111");
    expect(footprint.ref).toBe("R1");
    expect(footprint.value).toBe("10k");
    expect(footprint.footprint).toBe("Resistor_SMD:R_0402_1005Metric");
    expect(footprint.at).toEqual({ x: 25, y: 20, rotation: 90 });
    expect(footprint.layer).toBe("F.Cu");
    expect(footprint.pads).toEqual([
      { number: "1", netName: "+3V3", netNumber: 2 },
      { number: "2", netName: "GND", netNumber: 1 },
    ]);
  });

  it("KiCad 6 legacy 写法：fp_text 提供位号与值，tstamp 作为 id", () => {
    const document = parseKicadPcb(KICAD6_BOARD);

    expect(document.nets).toEqual([
      { number: 0, name: "" },
      { number: 1, name: "GND" },
    ]);
    // gr_line(0,0)-(50,0)、(50,0)-(50,30) 与 gr_circle 圆心(90,20) 半径 4 的外接框。
    expect(document.outline).toEqual({ width: 94, height: 30 });

    const footprint = document.footprints[0]!;
    expect(footprint.id).toBe("22222222-2222-2222-2222-222222222222");
    // fp_text user 不能顶替 reference/value。
    expect(footprint.ref).toBe("Q1");
    expect(footprint.value).toBe("2N7002");
    expect(footprint.footprint).toBe("Package_TO_SOT_SMD:SOT-23");
    // 没有 (at x y rot) 里的旋转时按 0 处理。
    expect(footprint.at).toEqual({ x: 10, y: 5, rotation: 0 });
    expect(footprint.layer).toBe("F.Cu");
    expect(footprint.pads).toEqual([
      // KiCad 6 的网名可以不加引号 (net 1 GND)，与加引号写法等价。
      { number: "1", netName: "GND", netNumber: 1 },
      // 没有 (net …) 的焊盘两个字段都是 undefined。
      { number: "2", netName: undefined, netNumber: undefined },
    ]);
  });

  it("字段缺失：缺 uuid/tstamp 时用确定性 id，无 Edge.Cuts 时不产出板框", () => {
    const document = parseKicadPcb(`(kicad_pcb (version 20240108) (generator "pcbnew")
      (net 1 "GND")
      (footprint "Package_DFN_QFN:QFN-24_4x4mm_P0.5mm"
        (layer "F.Cu")
        (at 12.5 7)
        (property "Reference" "U5" (at 0 0) (layer "F.SilkS") (uuid "u5-ref"))
        (pad "1" smd rect (at 0 0) (size 0.3 0.8) (layers "F.Cu" "F.Paste" "F.Mask"))
      )
      (footprint
        (at 1 2)
        (pad "" np_thru_hole circle (at 0 0) (size 3 3) (layers "*.Cu" "*.Mask"))
      )
    )`);

    expect(document.outline).toBeUndefined();
    expect(document.footprints).toHaveLength(2);

    const [first, second] = document.footprints;
    expect(first!.id).toBe("pcb:U5@12.5,7");
    expect(first!.ref).toBe("U5");
    expect(first!.value).toBeUndefined();
    expect(first!.pads).toEqual([{ number: "1", netName: undefined, netNumber: undefined }]);

    // 第二个封装没有位号、uuid、封装名与 layer：id 用确定性回退，字段按缺失省略。
    expect(second!.id).toBe("pcb:@1,2");
    expect(second!.ref).toBe("");
    expect(second!.value).toBeUndefined();
    expect(second!.footprint).toBeUndefined();
    expect(second!.layer).toBeUndefined();
    expect(second!.at).toEqual({ x: 1, y: 2, rotation: 0 });
    // NPTH 机械孔的编号本来就是空串。
    expect(second!.pads).toEqual([{ number: "", netName: undefined, netNumber: undefined }]);
  });

  it("板框：只有 Edge.Cuts 的图形项参与，gr_arc/gr_circle 按几何外接计算", () => {
    const document = parseKicadPcb(`(kicad_pcb (version 20240108) (generator "pcbnew")
      (gr_circle (center 20 0) (end 25 0) (stroke (width 0.05) (type default)) (fill none) (layer "Edge.Cuts") (uuid "c1"))
      (gr_arc (start 0 0) (mid 5 -5) (end 10 0) (stroke (width 0.05) (type default)) (layer "Edge.Cuts") (uuid "a1"))
      (gr_rect (start 100 100) (end 200 200) (stroke (width 0.05) (type default)) (fill none) (layer "F.SilkS") (uuid "s1"))
    )`);

    // circle：圆心 ± 半径 → x 15..25、y -5..5；arc：三个控制点 (0,0)(5,-5)(10,0)。
    // F.SilkS 的 rect(100,100)-(200,200) 被排除，否则宽度会变成 200。
    expect(document.outline).toEqual({ width: 25, height: 10 });
  });

  it("id 同时存在 uuid 与 tstamp 时优先 uuid", () => {
    const document = parseKicadPcb(`(kicad_pcb (version 20240108) (generator "pcbnew")
      (footprint "Resistor_SMD:R_0402_1005Metric" (layer "F.Cu")
        (uuid "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
        (tstamp 00000000-0000-0000-0000-000000000001)
        (at 0 0)
        (property "Reference" "R9" (at 0 0) (layer "F.SilkS"))
      )
    )`);

    expect(document.footprints[0]?.id).toBe("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
  });

  it("property 与 fp_text 同时存在时优先 property", () => {
    const document = parseKicadPcb(`(kicad_pcb (version 20240108) (generator "pcbnew")
      (footprint "Device:R" (layer "F.Cu")
        (at 0 0)
        (fp_text reference "旧位号" (at 0 0) (layer "F.SilkS"))
        (fp_text value "旧值" (at 0 0) (layer "F.Fab"))
        (property "Reference" "R7" (at 0 0) (layer "F.SilkS"))
        (property "Value" "22k" (at 0 0) (layer "F.Fab"))
      )
    )`);

    const footprint = document.footprints[0]!;
    expect(footprint.ref).toBe("R7");
    expect(footprint.value).toBe("22k");
  });

  it("顶层 head 不是 kicad_pcb：抛 unsupported_format 并带实际 head", () => {
    expect(() => parseKicadPcb(`(kicad_sch (version 20231120) (generator "eeschema"))`)).toThrow(
      /unsupported_format: 顶层 head 为 kicad_sch/,
    );
  });

  it("KiCad 5 legacy .brd（非 s-expression）：抛带说明的 unsupported_format", () => {
    const legacyBrd = `PCBNEW-BOARD Version 4 date Mon 01 Jan 2018 00:00:00
$GENERAL
  Links 0  NoConn 0  Diags 0
$MODULE DIP-8_W7.62mm
Po 3000 2000 0 15 4D5F3C46 00000000 ~~
$EndMODULE DIP-8_W7.62mm
$EndBOARD`;

    expect(() => parseKicadPcb(legacyBrd)).toThrow(/unsupported_format.*不是 KiCad s-expression/);
  });
});
