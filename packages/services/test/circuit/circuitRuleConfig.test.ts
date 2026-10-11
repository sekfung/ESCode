import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { CIRCUIT_RULE_CONFIG_PATH } from "@escode/shared/circuit";
import { createCircuitService, type CircuitService } from "../../src/circuit/app/circuitService.js";
import { createFsCircuitStore } from "../../src/circuit/adapters/fsCircuitStore.js";
import { createFsSourceWatcher } from "../../src/circuit/adapters/sourceWatcher.js";
import { createWorkspaceSource } from "../../src/circuit/adapters/workspaceFiles.js";
import { parseCircuitRuleConfig } from "../../src/circuit/domain/circuitRuleConfig.js";

/**
 * 规则配置（规格 §6.1）：`.escode/circuit-rules.json` 只读；不合法整份忽略并在结果里报告。
 * 这里用临时 workspace 走真实文件读取，配置解析本身用纯函数直接覆盖各种非法结构。
 */

/** 一个电阻，pin1 接标签 SENSE、pin2 悬空 → 默认配置下 unconnected-pin 与 single-pin-net 都命中。 */
const BOARD = `(kicad_sch (version 20231120) (generator eeschema) (uuid 0f000000-0000-4000-8000-00000000000a) (paper "A4")
  (lib_symbols
    (symbol "Device:R" (pin_numbers hide) (pin_names (offset 0)) (exclude_from_sim no) (in_bom yes) (on_board yes)
      (property "Reference" "R" (at 2.032 0 90) (effects (font (size 1.27 1.27))))
      (property "Value" "R" (at 0 0 90) (effects (font (size 1.27 1.27))))
      (symbol "R_0_1" (rectangle (start -1.016 -2.54) (end 1.016 2.54)))
      (symbol "R_1_1"
        (pin passive line (at 0 3.81 270) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27)))))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" (effects (font (size 1.27 1.27)))) (number "2" (effects (font (size 1.27 1.27)))))))
  )
  (symbol (lib_id "Device:R") (at 100 100 0) (unit 1) (exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no) (uuid 0f000000-0000-4000-8000-00000000000b)
    (property "Reference" "R1" (at 102 99 0) (effects (font (size 1.27 1.27))))
    (property "Value" "10k" (at 102 101 0) (effects (font (size 1.27 1.27))))
    (pin "1" (uuid 0f000000-0000-4000-8000-00000000000c)) (pin "2" (uuid 0f000000-0000-4000-8000-00000000000d))
    (instances (project "g3-fixture" (path "/0f000000-0000-4000-8000-00000000000a" (reference "R1") (unit 1)))))
  (wire (pts (xy 100 96.19) (xy 100 90)) (stroke (width 0) (type default)) (uuid 0f000000-0000-4000-8000-00000000000e))
  (label "SENSE" (at 100 90 0) (effects (font (size 1.27 1.27)) (justify left bottom)) (uuid 0f000000-0000-4000-8000-00000000000f))
)
`;

describe("规则配置解析（纯函数）", () => {
  it("合法配置覆盖 enabled / severity，未出现的规则用默认值", () => {
    const parsed = parseCircuitRuleConfig(
      JSON.stringify({
        rules: {
          "missing-decoupling": { enabled: false },
          "i2c-missing-pullup": { severity: "error" },
          "net-name-conflict": { enabled: true, severity: "info" },
        },
      }),
    );
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.settings["missing-decoupling"]).toEqual({ enabled: false, severity: "info" });
    expect(parsed.settings["i2c-missing-pullup"]).toEqual({ enabled: true, severity: "error" });
    expect(parsed.settings["net-name-conflict"]).toEqual({ enabled: true, severity: "info" });
    expect(parsed.settings["unconnected-pin"]).toEqual({ enabled: true, severity: "warning" });
  });

  it("空对象与省略 rules 都是合法配置", () => {
    expect(parseCircuitRuleConfig("{}").ok).toBe(true);
    expect(parseCircuitRuleConfig('{"rules":{}}').ok).toBe(true);
  });

  const invalid: [string, string, string][] = [
    ["不是 JSON", "not json", "JSON 解析失败"],
    ["根不是对象", "[]", "必须是对象"],
    ["未知顶层字段", '{"rules":{},"extra":1}', '不支持的字段 "extra"'],
    ["rules 不是对象", '{"rules":[]}', '"rules" 必须是对象'],
    ["未知规则 id", '{"rules":{"custom-rule":{}}}', "未知规则"],
    ["规则配置不是对象", '{"rules":{"missing-decoupling":true}}', "必须是对象"],
    ["规则字段不认识", '{"rules":{"missing-decoupling":{"level":"warn"}}}', "不支持的字段"],
    ["enabled 不是布尔", '{"rules":{"missing-decoupling":{"enabled":"no"}}}', "必须是布尔值"],
    ["severity 取值非法", '{"rules":{"missing-decoupling":{"severity":"fatal"}}}', "必须是 info"],
  ];
  it.each(invalid)("%s → 整份配置作废并给出原因", (_name, text, reason) => {
    const parsed = parseCircuitRuleConfig(text);
    expect(parsed.ok).toBe(false);
    if (parsed.ok) return;
    expect(parsed.reason).toContain(reason);
  });
});

describe("规则配置读取与检查结果（§6.1）", () => {
  let service: CircuitService;
  let workspace: string;
  let irId: string;

  async function writeConfig(text: string): Promise<void> {
    await mkdir(join(workspace, ".escode"), { recursive: true });
    await writeFile(join(workspace, CIRCUIT_RULE_CONFIG_PATH), text, "utf8");
  }

  beforeAll(async () => {
    workspace = await mkdtemp(join(tmpdir(), "circuit-config-"));
    service = createCircuitService({
      ports: {
        store: createFsCircuitStore({ rootDir: join(workspace, ".data") }),
        source: createWorkspaceSource(),
        watcher: createFsSourceWatcher(),
      },
    });
    await writeFile(join(workspace, "board.kicad_sch"), BOARD, "utf8");
    const view = await service.parse({ workspacePath: workspace, path: "board.kicad_sch" });
    irId = view.summary.irId;
  });

  afterAll(async () => {
    service.dispose();
    await rm(workspace, { recursive: true, force: true });
  });

  it("没有配置文件时 status=default，按默认级别给出条目", async () => {
    const result = await service.check({ workspacePath: workspace, irId });
    expect(result.config).toEqual({ status: "default" });
    const unconnected = result.items.filter((item) => item.ruleId === "unconnected-pin");
    expect(unconnected).toHaveLength(1);
    expect(unconnected[0]!.severity).toBe("warning");
  });

  it("合法配置时 status=applied：停用的规则不出现，级别按配置覆盖", async () => {
    await writeConfig(
      JSON.stringify({
        rules: {
          "unconnected-pin": { enabled: false },
          "single-pin-net": { severity: "error" },
        },
      }),
    );
    const result = await service.check({ workspacePath: workspace, irId });
    expect(result.config).toEqual({ status: "applied", path: CIRCUIT_RULE_CONFIG_PATH });
    expect(result.items.some((item) => item.ruleId === "unconnected-pin")).toBe(false);
    // R1.2 悬空（未命名单引脚网络）+ R1.1 接在具名网络 SENSE 上：两个单引脚网络都按配置升级为 error。
    const single = result.items.filter((item) => item.ruleId === "single-pin-net");
    expect(single).toHaveLength(2);
    expect(single.every((item) => item.severity === "error")).toBe(true);
  });

  it("配置不合法时整份忽略、报告 invalid 与原因，并回退到默认配置", async () => {
    await writeConfig('{"rules":{"not-a-rule":{}}}');
    const invalid = await service.check({ workspacePath: workspace, irId });
    expect(invalid.config.status).toBe("invalid");
    expect(invalid.config.path).toBe(CIRCUIT_RULE_CONFIG_PATH);
    expect(invalid.config.reason).toContain("未知规则");
    expect(invalid.items.some((item) => item.ruleId === "unconnected-pin")).toBe(true);

    await writeConfig("{ this is not json");
    const broken = await service.check({ workspacePath: workspace, irId });
    expect(broken.config.status).toBe("invalid");
    expect(broken.config.reason).toContain("JSON 解析失败");
    expect(broken.items.some((item) => item.ruleId === "unconnected-pin")).toBe(true);
  });

  it("配置路径是目录时按「没有配置文件」处理：status=default，检查照常给出条目", async () => {
    // CircuitSourcePort.exists 只认常规文件（adapters/workspaceFiles.ts），目录不算配置文件。
    await rm(join(workspace, CIRCUIT_RULE_CONFIG_PATH), { force: true });
    await mkdir(join(workspace, CIRCUIT_RULE_CONFIG_PATH), { recursive: true });
    const result = await service.check({ workspacePath: workspace, irId });
    expect(result.config).toEqual({ status: "default" });
    expect(result.items.some((item) => item.ruleId === "unconnected-pin")).toBe(true);
    await rm(join(workspace, CIRCUIT_RULE_CONFIG_PATH), { recursive: true, force: true });
  });

  it("规则检查不需要 baseRevision，也不改变 IR 的 revision", async () => {
    const before = await service.get({ workspacePath: workspace, irId });
    await service.check({ workspacePath: workspace, irId });
    const after = await service.get({ workspacePath: workspace, irId });
    expect(after.summary.revision).toBe(before.summary.revision);
  });
});
