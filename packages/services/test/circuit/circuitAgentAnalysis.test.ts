import { describe, expect, it } from "vitest";
import {
  circuitToolResponseSchemas,
  escodeCircuitMethods,
  type ESCodeCircuitMethodParams,
} from "@escode/shared/circuit";
import type { ICircuitService } from "../../src/circuit/contract.js";
import { createCircuitAgentBridge } from "../../src/circuit/app/circuitAgentBridge.js";
import { routeCircuitAgentRequest } from "../../src/circuit/app/circuitAgentRequestRouter.js";
import { CircuitServiceError } from "../../src/circuit/domain/circuitErrors.js";

/**
 * 只读分析工具（固件比对）的 Agent 链路契约（docs/circuit/README.md §5、§7）：
 * op → service.compareFirmware 的映射、cursor 与 files 的透传、错误码 → 失败信封，
 * 以及反向路由的严格校验与响应复核。
 * 读写 op（parse/get/query/correct/export）的同类契约见 circuitAgentBridge.test.ts。
 */

const firmwareDifference = {
  kind: "pin-mismatch",
  pin: "PA9",
  netName: "LED2",
  firmwareSymbol: "LED1",
  file: "firmware/board.dts",
  line: 21,
} as const;

/** 分析 handler 只调用同名服务方法：这里只桩出这个方法，其余成员不参与这条路径。 */
function serviceWith(impl: Partial<ICircuitService>): ICircuitService {
  return impl as ICircuitService;
}

const methodParams = <Op extends keyof typeof escodeCircuitMethods>(
  _op: Op,
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

describe("只读分析工具的桥接", () => {
  it("compareFirmware 映射到服务方法，并透传 cursor 与 files", async () => {
    const calls: Array<{ method: string; params: unknown }> = [];
    const bridge = createCircuitAgentBridge({
      getCircuitService: () =>
        serviceWith({
          async compareFirmware(params) {
            calls.push({ method: "compareFirmware", params });
            return {
              items: [firmwareDifference],
              ...(params.cursor ? {} : { nextCursor: "c-fw" }),
            };
          },
        }),
    });

    expect(
      await bridge.handle(
        "compareFirmware",
        methodParams("compareFirmware", {
          irId: "ir:1",
          files: ["firmware/board.dts"],
          cursor: "opaque-0",
        }),
      ),
    ).toEqual({ ok: true, items: [firmwareDifference] });
    expect(calls[0]).toEqual({
      method: "compareFirmware",
      params: {
        workspacePath: "C:/work",
        workspaceIdentity: "identity-1",
        irId: "ir:1",
        files: ["firmware/board.dts"],
        cursor: "opaque-0",
      },
    });

    const firstPage = await bridge.handle(
      "compareFirmware",
      methodParams("compareFirmware", { irId: "ir:1", files: ["firmware/board.dts"] }),
    );
    expect(firstPage).toEqual({ ok: true, items: [firmwareDifference], nextCursor: "c-fw" });
  });

  it("服务错误码原样进入失败信封，消息去掉重复前缀", async () => {
    const bridge = createCircuitAgentBridge({
      getCircuitService: () =>
        serviceWith({
          async compareFirmware() {
            throw new CircuitServiceError("unsupported_format", "不支持的固件文件：fw/main.c");
          },
        }),
    });

    expect(
      await bridge.handle(
        "compareFirmware",
        methodParams("compareFirmware", { irId: "ir:1", files: ["fw/main.c"] }),
      ),
    ).toEqual({
      ok: false,
      error: { code: "unsupported_format", message: "不支持的固件文件：fw/main.c" },
    });
  });
});

describe("分析 op 的反向路由", () => {
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

  it("非法参数 -32602 不触桥接；成功时按工具总表复核后原样应答", async () => {
    const { responder, responded, errors } = createResponder();
    let called = 0;
    const bridge = createCircuitAgentBridge({
      getCircuitService: () =>
        serviceWith({
          async compareFirmware() {
            called += 1;
            return { items: [firmwareDifference], nextCursor: "c-fw" };
          },
        }),
    });

    routeCircuitAgentRequest({
      request: {
        id: 1,
        method: escodeCircuitMethods.compareFirmware,
        // 绝对路径不符合 workspace 相对路径契约。
        params: methodParams("compareFirmware", { irId: "ir:1", files: ["/abs/firmware.dts"] }),
      },
      bridge,
      responder,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect((errors[0] as { code: number }).code).toBe(-32602);
    expect(called).toBe(0);

    routeCircuitAgentRequest({
      request: {
        id: 2,
        method: escodeCircuitMethods.compareFirmware,
        params: methodParams("compareFirmware", { irId: "ir:1", files: ["firmware/board.dts"] }),
      },
      bridge,
      responder,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(responded[0]).toEqual({ ok: true, items: [firmwareDifference], nextCursor: "c-fw" });
    expect(circuitToolResponseSchemas.compareFirmware.safeParse(responded[0]).success).toBe(true);
  });

  it("响应不合契约时按 -32603 报告，不静默透传", async () => {
    const { responder, responded, errors } = createResponder();
    routeCircuitAgentRequest({
      request: {
        id: 1,
        method: escodeCircuitMethods.compareFirmware,
        params: methodParams("compareFirmware", { irId: "ir:1", files: ["firmware/board.dts"] }),
      },
      bridge: createCircuitAgentBridge({
        getCircuitService: () =>
          serviceWith({
            async compareFirmware() {
              // 结构缺陷：kind 不在枚举里。
              return { items: [{ kind: "nonsense", file: "firmware/board.dts" }] } as never;
            },
          }),
      }),
      responder,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(responded).toEqual([]);
    expect((errors[0] as { code: number }).code).toBe(-32603);
  });
});
