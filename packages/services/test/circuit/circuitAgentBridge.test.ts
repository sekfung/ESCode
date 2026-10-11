import { describe, expect, it } from "vitest";
import { Emitter } from "@escode/rpc";
import {
  escodeCircuitMethods,
  circuitQueryResponseSchema,
  type ESCodeCircuitMethodParams,
} from "@escode/shared/circuit";
import type { CircuitIrView, ICircuitService } from "../../src/circuit/contract.js";
import { createCircuitAgentBridge } from "../../src/circuit/app/circuitAgentBridge.js";
import { routeCircuitAgentRequest } from "../../src/circuit/app/circuitAgentRequestRouter.js";
import { CircuitServiceError } from "../../src/circuit/domain/circuitErrors.js";

/**
 * G4 契约测试（docs/circuit/README.md §5、§10.2）：
 * 工具 op → 服务方法的映射、错误码映射、cursor 透传、缺桥接/缺服务时的 unavailable，
 * 以及反向请求路由的严格校验。
 */

const view: CircuitIrView = {
  summary: {
    irId: "ir:1",
    revision: 2,
    source: { kind: "kicad_sch", path: "board.kicad_sch", contentHash: "0".repeat(64) },
    stale: false,
    componentCount: 1,
    netCount: 1,
    blockCount: 0,
    lowConfidenceCount: 0,
    diagnostics: { total: 1, error: 0, warning: 1, info: 0 },
  },
  diagnostics: [
    { code: "single-pin-net", severity: "warning", message: "网络只连了一个引脚", netIds: ["net:MID"] },
  ],
  truncated: false,
};

function createStubService(overrides: Partial<ICircuitService> = {}) {
  const calls: Array<{ method: string; params: unknown }> = [];
  const changed = new Emitter<never>();
  const service: ICircuitService = {
    onChanged: changed.event,
    async parse(params) {
      calls.push({ method: "parse", params });
      return view;
    },
    async findBySource() {
      return undefined;
    },
    async get(params) {
      calls.push({ method: "get", params });
      return view;
    },
    async getLayout() {
      return undefined;
    },
    async query(params) {
      calls.push({ method: "query", params });
      return { by: "ref", items: [], ...(params.cursor ? { nextCursor: "c-1" } : {}) };
    },
    async correct(params) {
      calls.push({ method: "correct", params });
      return view;
    },
    async listCorrections() {
      return [];
    },
    async dropCorrection() {
      return view;
    },
    async exportSnapshot(params) {
      calls.push({ method: "export", params });
      return { path: params.path };
    },
    ...overrides,
  };
  return { service, calls };
}

const methodParams = <Op extends keyof typeof escodeCircuitMethods>(
  op: Op,
  args: unknown,
): ESCodeCircuitMethodParams<Op> =>
  ({
    requestId: "req-1",
    sessionId: "session-a",
    workspaceKey: "identity-1",
    workspacePath: "C:/work",
    workspaceIdentity: "identity-1",
    args,
  }) as ESCodeCircuitMethodParams<Op>;

describe("CircuitAgentBridge", () => {
  it("把工具参数与服务方法一一对应，并把 workspace 身份带给服务", async () => {
    const { service, calls } = createStubService();
    const bridge = createCircuitAgentBridge({ getCircuitService: () => service });

    const parsed = await bridge.handle("parse", methodParams("parse", { path: "board.kicad_sch" }));
    expect(parsed).toEqual({ ok: true, summary: view.summary });
    expect(calls[0]).toEqual({
      method: "parse",
      params: {
        workspacePath: "C:/work",
        workspaceIdentity: "identity-1",
        path: "board.kicad_sch",
      },
    });

    const got = await bridge.handle("get", methodParams("get", { irId: "ir:1" }));
    expect(got).toEqual({
      ok: true,
      summary: view.summary,
      diagnostics: view.diagnostics,
      truncated: false,
    });

    const corrected = await bridge.handle(
      "correct",
      methodParams("correct", {
        irId: "ir:1",
        baseRevision: 2,
        ops: [{ op: "update-component", componentId: "r1", value: "4k7" }],
      }),
    );
    expect(corrected).toEqual({ ok: true, summary: view.summary });
    // §5：Agent 修正记录 author=agent，带发起会话 id。
    expect((calls.at(-1)!.params as { author: unknown }).author).toEqual({
      kind: "agent",
      sessionId: "session-a",
    });

    const exported = await bridge.handle(
      "export",
      methodParams("export", { irId: "ir:1", path: "out/board.circuit.json" }),
    );
    expect(exported).toEqual({ ok: true, path: "out/board.circuit.json" });
  });

  it("query 的 cursor 原样透传，nextCursor 原样回给调用方", async () => {
    const { service, calls } = createStubService();
    const bridge = createCircuitAgentBridge({ getCircuitService: () => service });

    const result = await bridge.handle(
      "query",
      methodParams("query", { irId: "ir:1", by: "ref", ref: "R1", cursor: "opaque-cursor" }),
    );
    expect((calls[0]!.params as { cursor: string }).cursor).toBe("opaque-cursor");
    expect(result).toEqual({ ok: true, by: "ref", items: [], nextCursor: "c-1" });
    expect(circuitQueryResponseSchema.safeParse(result).success).toBe(true);
  });

  it("CircuitServiceError 的错误码原样进入失败信封，消息去掉重复前缀", async () => {
    const { service } = createStubService({
      async correct() {
        throw new CircuitServiceError("stale_revision", "当前 revision 为 3，请重读后再写入");
      },
    });
    const bridge = createCircuitAgentBridge({ getCircuitService: () => service });
    expect(
      await bridge.handle(
        "correct",
        methodParams("correct", {
          irId: "ir:1",
          baseRevision: 2,
          ops: [{ op: "update-component", componentId: "r1", value: "4k7" }],
        }),
      ),
    ).toEqual({
      ok: false,
      error: { code: "stale_revision", message: "当前 revision 为 3，请重读后再写入" },
    });
  });

  it("没有 CircuitService 时所有工具都返回 unavailable", async () => {
    const bridge = createCircuitAgentBridge({ getCircuitService: () => undefined });
    const result = await bridge.handle("get", methodParams("get", { irId: "ir:1" }));
    expect(result).toMatchObject({ ok: false, error: { code: "unavailable" } });
  });
});

describe("routeCircuitAgentRequest", () => {
  function createResponder() {
    const responded: unknown[] = [];
    const errors: unknown[] = [];
    return {
      responded,
      errors,
      responder: {
        respond: async (_id: string | number, result: unknown) => {
          responded.push(result);
        },
        respondError: async (
          _id: string | number,
          error: { code: number; message: string; data?: unknown },
        ) => {
          errors.push(error);
        },
      },
    };
  }

  it("非电路方法不拦截，交给后续分支", () => {
    const { responder } = createResponder();
    expect(
      routeCircuitAgentRequest({
        request: { id: 1, method: "interaction/serialRead", params: {} },
        bridge: undefined,
        responder,
      }),
    ).toBe(false);
  });

  it("参数不合协议时以 -32602 拒绝，不调用桥接", async () => {
    const { responder, errors } = createResponder();
    const { service, calls } = createStubService();
    const bridge = createCircuitAgentBridge({ getCircuitService: () => service });
    expect(
      routeCircuitAgentRequest({
        request: {
          id: 1,
          method: escodeCircuitMethods.parse,
          // 缺 workspacePath 与 sessionId
          params: { requestId: "req-1", workspaceKey: "k", args: { path: "board.kicad_sch" } },
        },
        bridge,
        responder,
      }),
    ).toBe(true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect((errors[0] as { code: number }).code).toBe(-32602);
    expect(calls).toEqual([]);
  });

  it("没有桥接时以 unavailable 工具失败应答（纯 CLI / 未装配电路工具的 Host）", async () => {
    const { responder, responded } = createResponder();
    routeCircuitAgentRequest({
      request: {
        id: "r-1",
        method: escodeCircuitMethods.get,
        params: {
          requestId: "req-1",
          sessionId: "session-a",
          workspaceKey: "k",
          workspacePath: "C:/work",
          args: { irId: "ir:1" },
        },
      },
      bridge: undefined,
      responder,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(responded[0]).toMatchObject({ ok: false, error: { code: "unavailable" } });
  });

  it("校验成功响应后原样应答，响应结构非法时按 -32603 报告", async () => {
    const { responder, responded, errors } = createResponder();
    const { service } = createStubService({
      async get() {
        // 结构缺陷：summary 不是合法摘要
        return { summary: { irId: "ir:1" }, diagnostics: [], truncated: false } as never;
      },
    });
    const bridge = createCircuitAgentBridge({ getCircuitService: () => service });
    const params = {
      requestId: "req-1",
      sessionId: "session-a",
      workspaceKey: "k",
      workspacePath: "C:/work",
      args: { irId: "ir:1" },
    };
    routeCircuitAgentRequest({
      request: { id: 1, method: escodeCircuitMethods.get, params },
      bridge,
      responder,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(responded).toEqual([]);
    expect((errors[0] as { code: number }).code).toBe(-32603);

    const healthy = createStubService();
    routeCircuitAgentRequest({
      request: { id: 2, method: escodeCircuitMethods.get, params },
      bridge: createCircuitAgentBridge({ getCircuitService: () => healthy.service }),
      responder,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(responded[0]).toMatchObject({ ok: true, summary: { irId: "ir:1" } });
  });
});
