import { mkdtemp, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { CircuitRuleResult } from "@escode/shared/circuit";
import { createCircuitService, type CircuitService } from "../../src/circuit/app/circuitService.js";
import { createFsCircuitStore } from "../../src/circuit/adapters/fsCircuitStore.js";
import { createFsSourceWatcher } from "../../src/circuit/adapters/sourceWatcher.js";
import { createWorkspaceSource } from "../../src/circuit/adapters/workspaceFiles.js";
import { defaultCircuitRuleSettings } from "../../src/circuit/domain/circuitRuleConfig.js";
import { runCircuitRules } from "../../src/circuit/domain/circuitRuleChecks.js";
import { decodeCircuitCursor, encodeCircuitCursor } from "../../src/circuit/app/circuitCursor.js";
import { pageByByteBudget } from "../../src/circuit/app/analysisPagination.js";
import { assembleCircuitIr } from "../../src/circuit/domain/irBuild.js";

/**
 * 内置规则（规格 §6、§10.2）：每条规则用 fixtures/circuit/rules/<ruleId>/{hit,miss}.kicad_sch
 * 走真实解析链路（parse → check），命中与不命中都断言。
 */

const fixturesDir = fileURLToPath(new URL("../fixtures/circuit/", import.meta.url));

function ruleItems(items: CircuitRuleResult[], ruleId: string): CircuitRuleResult[] {
  return items.filter((item) => item.ruleId === ruleId);
}

describe("内置规则命中与不命中", () => {
  let service: CircuitService;
  let dataDir: string;

  beforeAll(async () => {
    dataDir = await mkdtemp(join(tmpdir(), "circuit-rules-"));
    service = createCircuitService({
      ports: {
        store: createFsCircuitStore({ rootDir: dataDir }),
        source: createWorkspaceSource(),
        watcher: createFsSourceWatcher(),
      },
    });
  });

  afterAll(async () => {
    service.dispose();
    await rm(dataDir, { recursive: true, force: true });
  });

  /** 同一 fixture 只解析一次：已有 IR 时重新解析必须带 baseRevision，这里复用 irId。 */
  const irIdByPath = new Map<string, string>();
  async function irIdFor(path: string): Promise<string> {
    const cached = irIdByPath.get(path);
    if (cached) return cached;
    const view = await service.parse({ workspacePath: fixturesDir, path });
    irIdByPath.set(path, view.summary.irId);
    return view.summary.irId;
  }

  async function check(ruleId: string, variant: "hit" | "miss") {
    const path = `rules/${ruleId}/${variant}.kicad_sch`;
    return service.check({ workspacePath: fixturesDir, irId: await irIdFor(path) });
  }

  it("默认级别与规格 §6 表格一致，且解析得到的 IR 不是低置信度", async () => {
    const settings = defaultCircuitRuleSettings();
    expect(settings["unconnected-pin"]).toEqual({ enabled: true, severity: "warning" });
    expect(settings["single-pin-net"]).toEqual({ enabled: true, severity: "warning" });
    expect(settings["undriven-power"]).toEqual({ enabled: true, severity: "error" });
    expect(settings["i2c-missing-pullup"]).toEqual({ enabled: true, severity: "warning" });
    expect(settings["missing-decoupling"]).toEqual({ enabled: true, severity: "info" });
    expect(settings["net-name-conflict"]).toEqual({ enabled: true, severity: "error" });

    const result = await check("unconnected-pin", "hit");
    expect(result.config).toEqual({ status: "default" });
    expect(result.items.every((item) => item.lowConfidence === false)).toBe(true);
  });

  it("unconnected-pin：未接任何东西的引脚命中，接进具名单引脚网络的引脚不命中", async () => {
    const hit = await check("unconnected-pin", "hit");
    const items = ruleItems(hit.items, "unconnected-pin");
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ severity: "warning", lowConfidence: false });
    // R1.1 接在具名网络 SENSE 上（只由 single-pin-net 报出），R1.2 什么都没接。
    expect(items[0]!.message).toContain("R1 的引脚 2");
    expect(items[0]!.componentIds).toHaveLength(1);
    expect(items[0]!.netIds).toHaveLength(1);

    const miss = await check("unconnected-pin", "miss");
    expect(ruleItems(miss.items, "unconnected-pin")).toEqual([]);
  });

  it("single-pin-net：单引脚网络命中，两个引脚以上的网络不命中", async () => {
    const hit = await check("single-pin-net", "hit");
    const items = ruleItems(hit.items, "single-pin-net");
    expect(items).toHaveLength(2);
    expect(items.map((item) => item.severity)).toEqual(["warning", "warning"]);
    expect(items.some((item) => item.message.includes("SENSE"))).toBe(true);
    expect(items.some((item) => item.message.includes("RET"))).toBe(true);

    const miss = await check("single-pin-net", "miss");
    expect(ruleItems(miss.items, "single-pin-net")).toEqual([]);
  });

  it("undriven-power：只有电源输入引脚与电容的电源网命中，有 VOUT 驱动的电源网不命中", async () => {
    const hit = await check("undriven-power", "hit");
    const items = ruleItems(hit.items, "undriven-power");
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ severity: "error" });
    expect(items[0]!.message).toContain("+3V3");
    expect(items[0]!.message).toContain("VDD");

    const miss = await check("undriven-power", "miss");
    expect(ruleItems(miss.items, "undriven-power")).toEqual([]);
  });

  it("i2c-missing-pullup：没有上拉的 SDA/SCL 命中，上拉到电源轨后不命中", async () => {
    const hit = await check("i2c-missing-pullup", "hit");
    const items = ruleItems(hit.items, "i2c-missing-pullup");
    expect(items).toHaveLength(2);
    expect(items.map((item) => item.severity)).toEqual(["warning", "warning"]);
    expect(items.some((item) => item.message.includes("I2C1_SDA"))).toBe(true);
    expect(items.some((item) => item.message.includes("I2C1_SCL"))).toBe(true);

    const miss = await check("i2c-missing-pullup", "miss");
    expect(ruleItems(miss.items, "i2c-missing-pullup")).toEqual([]);
  });

  it("missing-decoupling：电源网上没有接地的电容命中，有去耦电容不命中", async () => {
    const hit = await check("missing-decoupling", "hit");
    const items = ruleItems(hit.items, "missing-decoupling");
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ severity: "info" });
    expect(items[0]!.message).toContain("U2");
    expect(items[0]!.message).toContain("+3V3");

    const miss = await check("missing-decoupling", "miss");
    expect(ruleItems(miss.items, "missing-decoupling")).toEqual([]);
  });

  it("net-name-conflict：同名但互不连通的两个网络命中，单实例不命中", async () => {
    const hit = await check("net-name-conflict", "hit");
    const items = ruleItems(hit.items, "net-name-conflict");
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ severity: "error" });
    expect(items[0]!.message).toContain("CLK");
    expect(items[0]!.netIds).toHaveLength(2);

    const miss = await check("net-name-conflict", "miss");
    expect(ruleItems(miss.items, "net-name-conflict")).toEqual([]);
  });

  it("同一份 IR 重复检查得到同样的条目与顺序", async () => {
    const irId = await irIdFor("rules/net-name-conflict/hit.kicad_sch");
    const first = await service.check({ workspacePath: fixturesDir, irId });
    const second = await service.check({ workspacePath: fixturesDir, irId });
    expect(second).toEqual(first);
  });

  it("3 块开源板子的规则检查只产出合法条目（真实数据冒烟）", async () => {
    const boards: [string, string][] = [
      ["rp2040-minimal", "RP2040_minimal.kicad_sch"],
      ["usbc-buck-3v3", "USB power port.kicad_sch"],
      ["mpu6050-imu", "MPU6050.kicad_sch"],
    ];
    for (const [dir, file] of boards) {
      const path = `${dir}/${file}`;
      const view = await service.parse({ workspacePath: fixturesDir, path });
      const result = await service.check({ workspacePath: fixturesDir, irId: view.summary.irId });
      for (const item of result.items) {
        expect(item.message.length).toBeGreaterThan(0);
        expect(["info", "warning", "error"]).toContain(item.severity);
        expect(item.lowConfidence).toBe(false);
      }
      expect(new Set(result.items.map((item) => item.ruleId)).size).toBeLessThanOrEqual(6);
      // 真实板子文件名下没有配置文件 → 默认配置。
      expect(result.config.status).toBe("default");
    }
  }, 120_000);
});

describe("规则结果分页（§5 响应上限）", () => {
  it("按字节预算切页，游标绑定查询签名", () => {
    const items = Array.from({ length: 2000 }, (_, index) => ({ index, text: "x".repeat(64) }));
    const first = pageByByteBudget(items, 0);
    expect(first.page.length).toBeGreaterThan(0);
    expect(first.page.length).toBeLessThan(items.length);
    expect(first.hasMore).toBe(true);

    const cursor = encodeCircuitCursor("sig", first.page.length);
    expect(decodeCircuitCursor(cursor, "sig")).toBe(first.page.length);
    expect(() => decodeCircuitCursor(cursor, "other")).toThrow(/cursor/u);
    expect(() => decodeCircuitCursor("not-base64-url!", "sig")).toThrow(/cursor/u);

    const last = pageByByteBudget(items, items.length);
    expect(last.page).toEqual([]);
    expect(last.hasMore).toBe(false);
  });

  it("规则检查对低置信度元素标出 lowConfidence（§6）", () => {
    const ir = assembleCircuitIr({
      irId: "ir:low-confidence",
      revision: 1,
      source: { kind: "image", path: "photo.png", contentHash: "0".repeat(64) },
      stale: false,
      elements: {
        components: [
          {
            id: "u1",
            ref: "U1",
            pins: [{ number: "1", netId: "net:SENSE" }],
            origin: { by: "recognition", confidence: 0.4 },
          },
        ],
        nets: [
          {
            id: "net:SENSE",
            name: "SENSE",
            pinRefs: [{ componentId: "u1", pin: "1" }],
            origin: { by: "recognition", confidence: 0.4 },
          },
        ],
        blocks: [],
        diagnostics: [],
      },
    });
    const items = runCircuitRules(ir, defaultCircuitRuleSettings());
    const single = items.filter((item) => item.ruleId === "single-pin-net");
    expect(single).toHaveLength(1);
    expect(single[0]!.lowConfidence).toBe(true);
  });
});

describe("fixture 目录约定（§10.1）", () => {
  it("rules 目录下每个规则都有 hit / miss 两个 fixture", async () => {
    const ruleDirs = (await readdir(join(fixturesDir, "rules"), { withFileTypes: true }))
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort();
    expect(ruleDirs).toEqual([
      "i2c-missing-pullup",
      "missing-decoupling",
      "net-name-conflict",
      "single-pin-net",
      "unconnected-pin",
      "undriven-power",
    ]);
    for (const ruleId of ruleDirs) {
      const files = (await readdir(join(fixturesDir, "rules", ruleId))).sort();
      expect(files).toContain("hit.kicad_sch");
      expect(files).toContain("miss.kicad_sch");
    }
  });
});
