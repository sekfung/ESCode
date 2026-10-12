import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { CircuitIR } from "@escode/shared/circuit";
import { createCircuitService, type CircuitService } from "../../src/circuit/app/circuitService.js";
import { createFsCircuitStore } from "../../src/circuit/adapters/fsCircuitStore.js";
import { createFsSourceWatcher } from "../../src/circuit/adapters/sourceWatcher.js";
import { createWorkspaceSource } from "../../src/circuit/adapters/workspaceFiles.js";
import { parseFirmwarePins } from "../../src/circuit/domain/firmwareParse.js";
import { compareFirmwarePins } from "../../src/circuit/domain/firmwareCompare.js";
import { assembleCircuitIr } from "../../src/circuit/domain/irBuild.js";

/**
 * 固件比对（规格 §7、§7.1、§10.2）：
 * - 三种格式（devicetree / `.ioc` / C 头文件）各自的解析；
 * - 手工样例（fixtures/circuit/firmware/）与原理图的比对，覆盖四类差异；
 * - 错误码与分页游标；对照 IR 用零件拼装（不依赖解析器）。
 */

const fixturesDir = fileURLToPath(new URL("../fixtures/circuit/", import.meta.url));
const FIRMWARE_FILES = ["firmware/board.dts", "firmware/board.ioc", "firmware/board_pins.h"];

async function fixtureText(relativePath: string): Promise<string> {
  return readFile(join(fixturesDir, relativePath), "utf8");
}

describe("固件解析（devicetree / .ioc / 头文件）", () => {
  it("devicetree：节点标签 + 属性前缀组成信号名，行号指向属性", async () => {
    const declarations = parseFirmwarePins({
      path: "firmware/board.dts",
      text: await fixtureText("firmware/board.dts"),
    });
    expect(declarations.map((item) => [item.signal, item.pin, item.line])).toEqual([
      ["LED0", "PB6", 17],
      ["LED1", "PA9", 21],
      ["UART2_TX", "PA2", 28],
      ["I2C1_SDA", "PB7", 34],
      ["I2C1_SCL", "PB8", 35],
    ]);
  });

  it("devicetree：.dtsi / .overlay 走同一解析器，认不出的构造跳过", () => {
    const text = `#include <dt-bindings/gpio/gpio.h>
/ {
	aliases { console = &uart0; };
	status-led {
		led_gpios = <&gpioc 13 GPIO_ACTIVE_HIGH>;
	};
	sensor0: sensor@0 {
		io-gpios = <&gpio0 12 GPIO_ACTIVE_HIGH>;   /* 数字端口 → GPIO12 */
		psels = <NRF_PSEL(TWIM_SDA, 0, 26)>;
	};
};
&i2c0 {
	pinctrl-0 = <&i2c0_default>;
	pinctrl-names = "default";
};
`;
    for (const path of ["board.dtsi", "board.overlay"]) {
      const declarations = parseFirmwarePins({ path, text });
      expect(declarations.map((item) => [item.signal, item.pin])).toEqual([
        ["STATUS_LED", "PC13"],
        ["SENSOR0_IO", "GPIO12"],
      ]);
      expect(declarations[0]).toMatchObject({ symbol: "status-led", line: 5, file: path });
      // 数字端口控制器没有端口语义：两种写法都作为候选留给比对（Nordic P0.12 / ESP GPIO12）。
      expect(declarations[1]).toMatchObject({ pin: "GPIO12", pinAliases: ["P0.12"] });
      expect(declarations[0]!.pinAliases).toBeUndefined();
    }
  });

  it(".ioc：外设功能直接用，通用 GPIO 用 GPIO_Label，没有标签的通用 GPIO 跳过", async () => {
    const declarations = parseFirmwarePins({
      path: "firmware/board.ioc",
      text: await fixtureText("firmware/board.ioc"),
    });
    expect(declarations.map((item) => [item.signal, item.pin, item.line])).toEqual([
      ["UART2_TX", "PA2", 8],
      ["UART2_RX", "PA3", 11],
      ["LED0", "PB6", 14],
      ["I2C1_SDA", "PB7", 15],
    ]);
  });

  it("头文件：_PIN 与 _GPIO_PORT / _GPIO_Port / _PORT 组合，缺端口或非法端口跳过", () => {
    const text = `#define LED0_PIN GPIO_PIN_6
#define LED0_GPIO_Port GPIOB
#define NO_PORT_PIN GPIO_PIN_3
#define BAD_PORT_PIN GPIO_PIN_4
#define BAD_PORT_PORT NOT_A_PORT
#define I2C1_SCL_Pin LL_GPIO_PIN_8
#define I2C1_SCL_GPIO_PORT GPIOB
#define EMPTY_PIN 
`;
    const declarations = parseFirmwarePins({ path: "board_pins.h", text });
    expect(declarations.map((item) => [item.signal, item.pin, item.line])).toEqual([
      ["LED0", "PB6", 1],
      ["I2C1_SCL", "PB8", 6],
    ]);
  });

  it("头文件：CubeMX 样例的四种组合都能解析", async () => {
    const declarations = parseFirmwarePins({
      path: "firmware/board_pins.h",
      text: await fixtureText("firmware/board_pins.h"),
    });
    expect(declarations.map((item) => [item.signal, item.pin, item.line])).toEqual([
      ["LED0", "PB6", 13],
      ["UART2_TX", "PA2", 17],
      ["I2C1_SDA", "PB8", 22],
      ["I2C1_SCL", "PB8", 26],
    ]);
  });

  it("不支持的扩展名直接拒绝（Arduino / PlatformIO 不经过比对器）", () => {
    expect(() => parseFirmwarePins({ path: "main.ino", text: "" })).toThrow(/unsupported_format/u);
    expect(() => parseFirmwarePins({ path: "platformio.ini", text: "" })).toThrow(
      /unsupported_format/u,
    );
  });
});

/** 零件拼装的 IR：一个 MCU（U1），引脚名是 STM32 风格，便于直接断言比对结果。 */
function syntheticIr(): CircuitIR {
  const parserOrigin = { by: "parser", confidence: 1 } as const;
  const nets: CircuitIR["nets"] = [
    net("net:LED2", "LED2", [["u1", "6"]]),
    net("net:LED0", "LED0", [["u1", "7"], ["r1", "1"]]),
    net("net:I2C1_SDA", "I2C1_SDA", [["u1", "8"], ["r1", "2"]]),
    net("net:+3V3", "+3V3", [["u1", "1"]]),
    net("net:GND", "GND", [["u1", "2"]]),
    net("net:U1-Pad5", "Net-(U1-Pad5)", [["u1", "5"]]),
  ];
  function net(id: string, name: string, pins: [string, string][]): CircuitIR["nets"][number] {
    return {
      id,
      name,
      pinRefs: pins.map(([componentId, pin]) => ({ componentId, pin })),
      origin: parserOrigin,
    };
  }
  return assembleCircuitIr({
    irId: "ir:firmware-compare",
    revision: 1,
    source: { kind: "kicad_sch", path: "hardware/board.kicad_sch", contentHash: "0".repeat(64) },
    stale: false,
    elements: {
      components: [
        {
          id: "u1",
          ref: "U1",
          pins: [
            { number: "1", name: "VDD", netId: "net:+3V3" },
            { number: "2", name: "VSS", netId: "net:GND" },
            { number: "5", name: "PD0", netId: "net:U1-Pad5" },
            { number: "6", name: "PA9", netId: "net:LED2" },
            { number: "7", name: "PB6", netId: "net:LED0" },
            { number: "8", name: "PB7", netId: "net:I2C1_SDA" },
            { number: "9", name: "NRST" },
          ],
          origin: parserOrigin,
        },
        {
          id: "r1",
          ref: "R1",
          pins: [
            { number: "1", netId: "net:LED0" },
            { number: "2", netId: "net:I2C1_SDA" },
          ],
          origin: parserOrigin,
        },
      ],
      nets,
      blocks: [],
      diagnostics: [],
    },
  });
}

const declaration = (signal: string, pin: string, line = 1, file = "firmware/app.dts") => ({
  signal,
  pin,
  symbol: signal.toLowerCase(),
  file,
  line,
});

describe("固件比对（对照 IR）", () => {
  it("命中的声明不产出条目：网络名与信号名按同一规则规范化后比较", () => {
    const items = compareFirmwarePins(syntheticIr(), [
      declaration("LED0", "PB6"),
      declaration("I2C1_SDA", "PB7", 2),
      declaration("LED2", "PA9", 3),
    ]);
    expect(items).toEqual([]);

    // 原理图侧写成小写/连字符（如 Devicetree 风格的网名）时仍能匹配。
    const lowercaseNets = syntheticIr();
    lowercaseNets.nets = lowercaseNets.nets.map((net) =>
      net.id === "net:I2C1_SDA" ? { ...net, name: "i2c1-sda" } : net,
    );
    expect(
      compareFirmwarePins(lowercaseNets, [
        declaration("LED0", "PB6"),
        declaration("I2C1_SDA", "PB7", 2),
        declaration("LED2", "PA9", 3),
      ]),
    ).toEqual([]);
  });

  it("pin-mismatch 带 IR 侧的网络与器件，firmware-only 带固件声明位置", () => {
    const items = compareFirmwarePins(syntheticIr(), [
      declaration("LED0", "PB6"),
      declaration("I2C1_SDA", "PB7", 2),
      declaration("USART1_TX", "PA9", 7),
      declaration("UART3_TX", "PB12", 9),
    ]);
    expect(items).toEqual([
      {
        kind: "pin-mismatch",
        pin: "PA9",
        netId: "net:LED2",
        netName: "LED2",
        componentId: "u1",
        firmwareSymbol: "USART1_TX",
        file: "firmware/app.dts",
        line: 7,
      },
      {
        kind: "firmware-only",
        pin: "PB12",
        firmwareSymbol: "UART3_TX",
        file: "firmware/app.dts",
        line: 9,
      },
    ]);
  });

  it("schematic-only 只报具名信号网：电源、地与未命名网络不算", () => {
    const items = compareFirmwarePins(syntheticIr(), [declaration("LED0", "PB6")]);
    expect(items).toEqual([
      {
        kind: "schematic-only",
        pin: "PA9",
        netId: "net:LED2",
        netName: "LED2",
        componentId: "u1",
        file: "hardware/board.kicad_sch",
      },
      {
        kind: "schematic-only",
        pin: "PB7",
        netId: "net:I2C1_SDA",
        netName: "I2C1_SDA",
        componentId: "u1",
        file: "hardware/board.kicad_sch",
      },
    ]);
  });

  it("peripheral-conflict：同一引脚两个信号，或同一信号两个引脚（按排序在后的声明给出）", () => {
    const twoSignals = compareFirmwarePins(syntheticIr(), [
      declaration("LED0", "PB6", 3),
      declaration("LED1", "PB6", 4),
    ]);
    expect(twoSignals).toEqual([
      {
        kind: "peripheral-conflict",
        pin: "PB6",
        firmwareSymbol: "LED1",
        file: "firmware/app.dts",
        line: 4,
      },
      {
        kind: "pin-mismatch",
        pin: "PB6",
        netId: "net:LED0",
        netName: "LED0",
        componentId: "u1",
        firmwareSymbol: "LED1",
        file: "firmware/app.dts",
        line: 4,
      },
      {
        kind: "schematic-only",
        pin: "PA9",
        netId: "net:LED2",
        netName: "LED2",
        componentId: "u1",
        file: "hardware/board.kicad_sch",
      },
      {
        kind: "schematic-only",
        pin: "PB7",
        netId: "net:I2C1_SDA",
        netName: "I2C1_SDA",
        componentId: "u1",
        file: "hardware/board.kicad_sch",
      },
    ]);

    const twoPins = compareFirmwarePins(syntheticIr(), [
      declaration("I2C1_SDA", "PB7", 1, "firmware/app.dts"),
      declaration("I2C1_SDA", "PB12", 2, "firmware/app.dts"),
    ]);
    expect(twoPins.filter((item) => item.kind === "peripheral-conflict")).toEqual([
      { kind: "peripheral-conflict", pin: "PB12", firmwareSymbol: "I2C1_SDA", file: "firmware/app.dts", line: 2 },
    ]);
  });

  it("数字端口控制器的两种写法都能命中 IR 引脚名（Nordic P1.13 / ESP GPIO13）", () => {
    const parserOrigin = { by: "parser", confidence: 1 } as const;
    const ir = assembleCircuitIr({
      irId: "ir:numeric-port",
      revision: 1,
      source: { kind: "kicad_sch", path: "hardware/mcu.kicad_sch", contentHash: "0".repeat(64) },
      stale: false,
      elements: {
        components: [
          {
            id: "u1",
            ref: "U1",
            pins: [
              { number: "1", name: "P1.13", netId: "net:LED_N" },
              { number: "2", name: "IO13", netId: "net:LED_E" },
            ],
            origin: parserOrigin,
          },
        ],
        nets: [
          {
            id: "net:LED_N",
            name: "LED_N",
            pinRefs: [{ componentId: "u1", pin: "1" }],
            origin: parserOrigin,
          },
          {
            id: "net:LED_E",
            name: "LED_E",
            pinRefs: [{ componentId: "u1", pin: "2" }],
            origin: parserOrigin,
          },
        ],
        blocks: [],
        diagnostics: [],
      },
    });
    const declarations = parseFirmwarePins({
      path: "board.overlay",
      text: `/ {
	led_n { gpios = <&gpio1 13 GPIO_ACTIVE_HIGH>; };
	led_e { gpios = <&gpio0 13 GPIO_ACTIVE_HIGH>; };
};
`,
    });
    expect(declarations.map((item) => [item.signal, item.pin])).toEqual([
      ["LED_N", "GPIO13"],
      ["LED_E", "GPIO13"],
    ]);
    // 同一个「控制器 + 脚号」按信号名消歧：LED_N 落到 P1.13、LED_E 落到 IO13。
    expect(compareFirmwarePins(ir, declarations)).toEqual([]);
  });

  it("IR 里没有可识别的 MCU 引脚名时拒绝比对", () => {    const bomLikeIr = assembleCircuitIr({
      irId: "ir:bom",
      revision: 1,
      source: { kind: "bom", path: "bom.csv", contentHash: "0".repeat(64) },
      stale: false,
      elements: {
        components: [{ id: "bom:U1", ref: "U1", pins: [], origin: { by: "parser", confidence: 1 } }],
        nets: [],
        blocks: [],
        diagnostics: [],
      },
    });
    expect(() => compareFirmwarePins(bomLikeIr, [])).toThrow(/invalid_ir/u);
  });
});

describe("固件比对（服务层，手工样例 fixture）", () => {
  let service: CircuitService;
  let dataDir: string;
  let irId: string;

  beforeAll(async () => {
    dataDir = await mkdtemp(join(tmpdir(), "circuit-firmware-"));
    service = createCircuitService({
      ports: {
        store: createFsCircuitStore({ rootDir: dataDir }),
        source: createWorkspaceSource(),
        watcher: createFsSourceWatcher(),
      },
    });
    const view = await service.parse({
      workspacePath: fixturesDir,
      path: "firmware/schematic.kicad_sch",
    });
    irId = view.summary.irId;
  });

  afterAll(async () => {
    service.dispose();
    await rm(dataDir, { recursive: true, force: true });
  });

  it("dts + ioc + 头文件一起比对，覆盖四类差异且顺序稳定", async () => {
    const result = await service.compareFirmware({
      workspacePath: fixturesDir,
      irId,
      files: FIRMWARE_FILES,
    });
    expect(result.items.map((item) => item.kind)).toEqual([
      "pin-mismatch",
      "firmware-only",
      "firmware-only",
      "peripheral-conflict",
      "schematic-only",
    ]);
    expect(result.items).toEqual([
      {
        kind: "pin-mismatch",
        pin: "PA9",
        netName: "LED2",
        firmwareSymbol: "LED1",
        netId: expect.any(String),
        componentId: expect.any(String),
        file: "firmware/board.dts",
        line: 21,
      },
      {
        kind: "firmware-only",
        pin: "PB8",
        firmwareSymbol: "I2C1_SCL",
        file: "firmware/board.dts",
        line: 35,
      },
      {
        kind: "firmware-only",
        pin: "PB8",
        firmwareSymbol: "I2C1_SDA",
        file: "firmware/board_pins.h",
        line: 22,
      },
      {
        kind: "peripheral-conflict",
        pin: "PB8",
        firmwareSymbol: "I2C1_SDA",
        file: "firmware/board_pins.h",
        line: 22,
      },
      {
        kind: "schematic-only",
        pin: "PA4",
        netName: "UART2_CTS",
        netId: expect.any(String),
        componentId: expect.any(String),
        file: "firmware/schematic.kicad_sch",
      },
    ]);
  }, 60_000);

  it("每种格式单独传入都能比对（.ioc 无差异，头文件报冲突与固件独有）", async () => {
    const ioc = await service.compareFirmware({
      workspacePath: fixturesDir,
      irId,
      files: ["firmware/board.ioc"],
    });
    // .ioc 只声明了原理图上已存在且一致的信号与一个未声明的 PA3（UART2_RX 一致）：PA2/PA3/PB6/PB7 都匹配。
    expect(ioc.items.filter((item) => item.file === "firmware/board.ioc")).toEqual([]);

    const header = await service.compareFirmware({
      workspacePath: fixturesDir,
      irId,
      files: ["firmware/board_pins.h"],
    });
    expect(
      header.items.filter((item) => item.kind === "peripheral-conflict" || item.kind === "firmware-only"),
    ).toEqual([
      { kind: "firmware-only", pin: "PB8", firmwareSymbol: "I2C1_SDA", file: "firmware/board_pins.h", line: 22 },
      { kind: "firmware-only", pin: "PB8", firmwareSymbol: "I2C1_SCL", file: "firmware/board_pins.h", line: 26 },
      { kind: "peripheral-conflict", pin: "PB8", firmwareSymbol: "I2C1_SCL", file: "firmware/board_pins.h", line: 26 },
    ]);
  }, 60_000);

  it("游标与文件列表绑定；空文件列表与不支持的扩展名分别报 invalid_ir / unsupported_format", async () => {
    const first = await service.compareFirmware({
      workspacePath: fixturesDir,
      irId,
      files: FIRMWARE_FILES,
    });
    if (first.nextCursor) {
      await expect(
        service.compareFirmware({
          workspacePath: fixturesDir,
          irId,
          files: ["firmware/board.ioc"],
          cursor: first.nextCursor,
        }),
      ).rejects.toThrow(/cursor/u);
    }
    await expect(
      service.compareFirmware({ workspacePath: fixturesDir, irId, files: [] }),
    ).rejects.toThrow(/invalid_ir/u);
    await expect(
      service.compareFirmware({ workspacePath: fixturesDir, irId, files: ["firmware/notes.txt"] }),
    ).rejects.toThrow(/unsupported_format/u);
    await expect(
      service.compareFirmware({
        workspacePath: fixturesDir,
        irId,
        files: ["firmware/does-not-exist.dts"],
      }),
    ).rejects.toThrow(/not_found/u);
  }, 60_000);
});
