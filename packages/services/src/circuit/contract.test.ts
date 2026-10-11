import { describe, expect, it } from "vitest";
import { Emitter } from "@escode/rpc";
import {
  ICircuitService,
  type CircuitChangedEvent,
  type CircuitIrView,
  type CircuitLayout,
  type CircuitQueryResult,
} from "./contract.js";
import {
  exampleCircuitChangedEvent,
  exampleCircuitCheckResult,
  exampleCircuitCorrectParams,
  exampleCircuitCorrectionRecord,
  exampleCircuitExportParams,
  exampleCircuitExportResult,
  exampleCircuitFirmwareCompareParams,
  exampleCircuitFirmwareCompareResult,
  exampleCircuitIrView,
  exampleCircuitLayout,
  exampleCircuitParseParams,
  exampleCircuitQueryResult,
} from "./contract.example.js";

/**
 * contract 的消费方式：这里实现一个最小的内存版 ICircuitService，
 * 保证契约声明（方法名、参数、事件）能被真实调用方按预期使用；具体语义由服务测试覆盖。
 */
function createFakeService(): ICircuitService & { fireChanged: (event: CircuitChangedEvent) => void } {
  const changed = new Emitter<CircuitChangedEvent>();
  const view = (): CircuitIrView => structuredClone(exampleCircuitIrView);
  return {
    onChanged: changed.event,
    fireChanged: (event) => changed.fire(event),
    parse: async (params) => {
      if (params.path !== exampleCircuitParseParams.path) throw new Error("not_found: 未注册的源文件");
      return view();
    },
    findBySource: async () => ({
      irId: exampleCircuitIrView.summary.irId,
      revision: exampleCircuitIrView.summary.revision,
      stale: exampleCircuitIrView.summary.stale,
    }),
    get: async () => view(),
    getLayout: async (): Promise<CircuitLayout> => structuredClone(exampleCircuitLayout),
    query: async (): Promise<CircuitQueryResult> => structuredClone(exampleCircuitQueryResult),
    check: async () => structuredClone(exampleCircuitCheckResult),
    compareFirmware: async (params) => {
      if (params.files.length === 0) throw new Error("invalid_ir: 没有固件文件");
      return structuredClone(exampleCircuitFirmwareCompareResult);
    },
    correct: async (params) => {
      if (params.baseRevision !== exampleCircuitIrView.summary.revision)
        throw new Error("stale_revision: 版本落后");
      return { ...view(), summary: { ...view().summary, revision: params.baseRevision + 1 } };
    },
    listCorrections: async () => [{ index: 0, record: exampleCircuitCorrectionRecord }],
    dropCorrection: async () => view(),
    exportSnapshot: async (params) => ({ path: params.path }),
  };
}

describe("circuit-service 契约", () => {
  it("频道名与工具契约里的 MCP server 名一致", () => {
    expect(ICircuitService.channelName).toBe("circuit");
  });

  it("解析视图带摘要与诊断明细，布局按 revision 对齐", async () => {
    const service = createFakeService();
    const parsed = await service.parse(exampleCircuitParseParams);
    expect(parsed.summary.source).toEqual(exampleCircuitIrView.summary.source);
    expect(parsed.truncated).toBe(false);
    const layout = await service.getLayout({
      workspacePath: exampleCircuitParseParams.workspacePath,
      irId: parsed.summary.irId,
    });
    expect(layout?.revision).toBe(parsed.summary.revision);
    expect(layout?.sheets[0]?.symbols[0]?.elementId).toBe(exampleCircuitCorrectionRecord.target.id);
  });

  it("修正必须带 baseRevision，变更后 revision 递增", async () => {
    const service = createFakeService();
    const view = await service.correct(exampleCircuitCorrectParams);
    expect(view.summary.revision).toBe(exampleCircuitCorrectParams.baseRevision + 1);
    await expect(
      service.correct({ ...exampleCircuitCorrectParams, baseRevision: 99 }),
    ).rejects.toThrow(/stale_revision/u);
  });

  it("导出只经契约返回写入路径，调用方不接触文件系统", async () => {
    const service = createFakeService();
    await expect(service.exportSnapshot(exampleCircuitExportParams)).resolves.toEqual(
      exampleCircuitExportResult,
    );
  });

  it("onChanged 只描述事实：irId、revision 与 stale", async () => {
    const service = createFakeService();
    const seen: CircuitChangedEvent[] = [];
    const subscription = service.onChanged((event) => seen.push(event));
    service.fireChanged(exampleCircuitChangedEvent);
    expect(seen).toEqual([exampleCircuitChangedEvent]);
    subscription.dispose();
    service.fireChanged({ ...exampleCircuitChangedEvent, revision: 5 });
    expect(seen).toHaveLength(1);
  });

  it("规则检查结果带配置状态与低置信度标记", async () => {
    const service = createFakeService();
    const result = await service.check({
      workspacePath: exampleCircuitParseParams.workspacePath,
      irId: exampleCircuitIrView.summary.irId,
    });
    expect(result.config).toEqual({ status: "applied", path: ".escode/circuit-rules.json" });
    expect(result.items[0]?.ruleId).toBe("unconnected-pin");
    expect(result.items[0]?.lowConfidence).toBe(false);
  });

  it("固件比对返回带来源文件与行号的差异条目", async () => {
    const service = createFakeService();
    const result = await service.compareFirmware(exampleCircuitFirmwareCompareParams);
    expect(result.items[0]).toMatchObject({
      kind: "pin-mismatch",
      pin: "PA9",
      firmwareSymbol: "LED1",
      file: "firmware/app.overlay",
      line: 12,
    });
    await expect(
      service.compareFirmware({ ...exampleCircuitFirmwareCompareParams, files: [] }),
    ).rejects.toThrow(/invalid_ir/u);
  });
});
