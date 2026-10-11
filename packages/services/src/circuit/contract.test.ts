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
  exampleCircuitCorrectParams,
  exampleCircuitCorrectionRecord,
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
    correct: async (params) => {
      if (params.baseRevision !== exampleCircuitIrView.summary.revision)
        throw new Error("stale_revision: 版本落后");
      return { ...view(), summary: { ...view().summary, revision: params.baseRevision + 1 } };
    },
    listCorrections: async () => [{ index: 0, record: exampleCircuitCorrectionRecord }],
    dropCorrection: async () => view(),
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
});
