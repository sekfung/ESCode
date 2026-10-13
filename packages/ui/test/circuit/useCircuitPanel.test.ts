// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ICircuitService } from "@escode/services";
import { toCircuitIrSummary, type CircuitIR } from "@escode/shared/circuit";
import { useCircuitPanel } from "@/hooks/useCircuitPanel.js";

/**
 * 面板初始加载路径（规格 §8）：
 * - `::circuit` 卡片带来的 requestedIrId 直接读取；
 * - 没有 requestedIrId 时先按源文件找回已有 IR，KiCad 文件没有 IR 才请求确定性解析；
 * - 不属于面板入口的扩展名（如 PDF / 图片）按 unsupported 处理，不读也不解析。
 */

afterEach(cleanup);

const kicadIr: CircuitIR = {
  schemaVersion: 1,
  irId: "ir:kicad",
  revision: 1,
  source: { kind: "kicad_sch", path: "hw/board.kicad_sch", contentHash: "b".repeat(64) },
  stale: false,
  components: [],
  nets: [],
  blocks: [],
  diagnostics: [],
};

interface FakeServiceOptions {
  ir?: CircuitIR;
  found?: { irId: string; revision: number; stale: boolean };
  getError?: { code: string; message: string };
}

function createFakeService(options: FakeServiceOptions = {}) {
  const viewOf = (ir: CircuitIR) => ({
    summary: toCircuitIrSummary(ir),
    diagnostics: ir.diagnostics,
    truncated: false,
  });
  const get = vi.fn(async (params: { irId: string }) => {
    if (options.getError) throw options.getError;
    if (!options.ir) throw { code: "not_found", message: `IR ${params.irId} 不存在` };
    return viewOf(options.ir);
  });
  const findBySource = vi.fn(async () => options.found);
  const parse = vi.fn(async () => {
    if (!options.ir) throw { code: "not_found", message: "IR 不存在" };
    return viewOf(options.ir);
  });
  const getLayout = vi.fn(async () => undefined);
  const query = vi.fn(async (params: { by: string }) =>
    params.by === "ref"
      ? { by: "ref" as const, items: options.ir?.components ?? [] }
      : { by: "net" as const, items: options.ir?.nets ?? [] },
  );
  const listCorrections = vi.fn(async () => []);
  const onChanged = vi.fn(() => ({ dispose: () => {} }));
  const service = {
    get,
    findBySource,
    parse,
    getLayout,
    query,
    listCorrections,
    onChanged,
  } as unknown as ICircuitService;
  return { service, get, findBySource, parse, getLayout, query, listCorrections };
}

const workspace = { workspacePath: "/ws" };

describe("面板初始加载", () => {
  it("卡片带来的 irId 直接读取：不查 findBySource、不重新解析", async () => {
    const fake = createFakeService({ ir: kicadIr });
    const { result } = renderHook(() =>
      useCircuitPanel({
        service: fake.service,
        workspace,
        path: "hw/board.kicad_sch",
        requestedIrId: "ir:kicad",
        enabled: true,
      }),
    );
    await waitFor(() => expect(result.current.state.status).toBe("ready"));
    expect(result.current.state.data?.irId).toBe("ir:kicad");
    expect(result.current.state.data?.summary.source.kind).toBe("kicad_sch");
    expect(fake.get).toHaveBeenCalledWith(expect.objectContaining({ irId: "ir:kicad", workspacePath: "/ws" }));
    expect(fake.findBySource).not.toHaveBeenCalled();
    expect(fake.parse).not.toHaveBeenCalled();
  });

  it("已有 IR（findBySource 命中）时直接读取，不重新解析", async () => {
    const fake = createFakeService({
      ir: kicadIr,
      found: { irId: "ir:kicad", revision: 1, stale: false },
    });
    const { result } = renderHook(() =>
      useCircuitPanel({
        service: fake.service,
        workspace,
        path: "hw/board.kicad_sch",
        enabled: true,
      }),
    );
    await waitFor(() => expect(result.current.state.status).toBe("ready"));
    expect(result.current.state.data?.irId).toBe("ir:kicad");
    expect(fake.parse).not.toHaveBeenCalled();
  });

  it("KiCad 文件没有 IR 时走确定性解析（revision 从 r1 开始）", async () => {
    const fake = createFakeService({ ir: kicadIr });
    const { result } = renderHook(() =>
      useCircuitPanel({
        service: fake.service,
        workspace,
        path: "hw/board.kicad_sch",
        enabled: true,
      }),
    );
    await waitFor(() => expect(result.current.state.status).toBe("ready"));
    expect(fake.parse).toHaveBeenCalledWith(expect.objectContaining({ path: "hw/board.kicad_sch" }));
    expect(fake.get).toHaveBeenCalledWith(expect.objectContaining({ irId: "ir:kicad" }));
    expect(result.current.state.data?.revision).toBe(1);
  });

  it("不属于面板入口的扩展名按 unsupported 处理，不读也不解析", async () => {
    const fake = createFakeService({ ir: kicadIr });
    const { result } = renderHook(() =>
      useCircuitPanel({
        service: fake.service,
        workspace,
        path: "docs/schematic.png",
        enabled: true,
      }),
    );
    await waitFor(() => expect(result.current.state.status).toBe("unsupported"));
    expect(fake.findBySource).not.toHaveBeenCalled();
    expect(fake.get).not.toHaveBeenCalled();
    expect(fake.parse).not.toHaveBeenCalled();
  });

  it("卡片 irId 不存在时报 not_found，重试会重新读取", async () => {
    const fake = createFakeService({ getError: { code: "not_found", message: "IR ir:gone 不存在" } });
    const { result } = renderHook(() =>
      useCircuitPanel({
        service: fake.service,
        workspace,
        path: "hw/board.kicad_sch",
        requestedIrId: "ir:gone",
        enabled: true,
      }),
    );
    await waitFor(() => expect(result.current.state.status).toBe("empty"));
    expect(result.current.state.error?.code).toBe("not_found");
    act(() => result.current.retry());
    await waitFor(() => expect(fake.get).toHaveBeenCalledTimes(2));
    expect(result.current.state.status).toBe("empty");
  });
});
