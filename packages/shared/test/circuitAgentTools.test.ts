import { describe, expect, it } from "vitest";
import {
  CIRCUIT_AGENT_TOOL_OPS,
  CIRCUIT_BROKER_SOCKET_ENV,
  CIRCUIT_BROKER_TOKEN_ENV,
  ESCODE_HOST_CIRCUIT_ENV,
  circuitBrokerRequestSchema,
  circuitToolResponseSchemas,
  escodeCircuitMethodParamsSchemas,
  escodeCircuitMethods,
} from "../src/circuit/index.js";

/**
 * Agent 链路 schema（docs/circuit/README.md §5）：broker 信封、反向方法、连接材料 env。
 * 工具参数/响应本身由 circuitTools.test.ts 覆盖，这里只验证「协议层不放过任何多余字段」。
 */

const brokerBase = {
  id: "6f0a6d0e-0000-4000-8000-000000000001",
  token: "t".repeat(64),
  runtimeScope: "main" as const,
  sessionId: "session-a",
};

const protocolBase = {
  requestId: "req-1",
  sessionId: "session-a",
  workspaceKey: "C:/work",
  workspacePath: "C:/work",
};

describe("circuit broker / 反向协议", () => {
  it("env 常量与 serial 同构，且列出 §5 的全部六个 op", () => {
    expect([ESCODE_HOST_CIRCUIT_ENV, CIRCUIT_BROKER_SOCKET_ENV, CIRCUIT_BROKER_TOKEN_ENV]).toEqual([
      "ESCODE_HOST_CIRCUIT",
      "ESCODE_CIRCUIT_BROKER_SOCKET",
      "ESCODE_CIRCUIT_BROKER_TOKEN",
    ]);
    expect([...CIRCUIT_AGENT_TOOL_OPS]).toEqual([
      "parse",
      "get",
      "query",
      "correct",
      "compareFirmware",
      "export",
    ]);
    expect(Object.values(escodeCircuitMethods)).toEqual([
      "interaction/circuitParse",
      "interaction/circuitGet",
      "interaction/circuitQuery",
      "interaction/circuitCorrect",
      "interaction/circuitCompareFirmware",
      "interaction/circuitExport",
    ]);
  });

  it("broker 请求逐 op 严格校验参数（默认值与拒绝多余字段）", () => {
    expect(
      circuitBrokerRequestSchema.safeParse({ ...brokerBase, op: "parse", args: { path: "b.kicad_sch" } })
        .success,
    ).toBe(true);
    expect(
      circuitBrokerRequestSchema.safeParse({ ...brokerBase, op: "parse", args: {} }).success,
    ).toBe(false);
    expect(
      circuitBrokerRequestSchema.safeParse({
        ...brokerBase,
        op: "get",
        args: { irId: "ir:1", extra: true },
      }).success,
    ).toBe(false);
    // 分析类工具：compareFirmware 要 irId + files（+ 可选 cursor）。
    expect(
      circuitBrokerRequestSchema.safeParse({
        ...brokerBase,
        op: "compareFirmware",
        args: { irId: "ir:1", files: ["firmware/app.overlay"], cursor: "opaque-1" },
      }).success,
    ).toBe(true);
    expect(
      circuitBrokerRequestSchema.safeParse({
        ...brokerBase,
        op: "compareFirmware",
        args: { irId: "ir:1", files: [] },
      }).success,
    ).toBe(false);
    expect(
      circuitBrokerRequestSchema.safeParse({
        ...brokerBase,
        op: "get",
        args: { irId: "ir:1" },
        extra: 1,
      }).success,
    ).toBe(false);
  });

  it("反向方法参数是 workspace 相对请求：缺 sessionId / 绝对路径被拒绝", () => {
    expect(
      escodeCircuitMethodParamsSchemas.export.safeParse({
        ...protocolBase,
        workspaceIdentity: "identity-1",
        remoteSessionId: "remote-1",
        args: { irId: "ir:1", path: "out/board.circuit.json" },
      }).success,
    ).toBe(true);
    expect(
      escodeCircuitMethodParamsSchemas.export.safeParse({
        ...protocolBase,
        args: { irId: "ir:1", path: "/etc/board.circuit.json" },
      }).success,
    ).toBe(false);
    expect(
      escodeCircuitMethodParamsSchemas.get.safeParse({
        requestId: "req-1",
        workspaceKey: "C:/work",
        workspacePath: "C:/work",
        args: { irId: "ir:1" },
      }).success,
    ).toBe(false);
  });

  it("分析类工具的反向参数同样严格：cursor 合法、绝对固件路径被拒", () => {
    expect(
      escodeCircuitMethodParamsSchemas.compareFirmware.safeParse({
        ...protocolBase,
        workspaceIdentity: "identity-1",
        remoteSessionId: "remote-1",
        args: { irId: "ir:1", files: ["firmware/app.overlay"], cursor: "opaque-1" },
      }).success,
    ).toBe(true);
    expect(
      escodeCircuitMethodParamsSchemas.compareFirmware.safeParse({
        ...protocolBase,
        args: { irId: "ir:1", files: ["/etc/app.overlay"] },
      }).success,
    ).toBe(false);
  });

  it("工具失败信封与响应总表一致（错误码固定七个）", () => {
    const failure = { ok: false, error: { code: "stale_revision", message: "版本落后" } };
    expect(circuitToolResponseSchemas.correct.safeParse(failure).success).toBe(true);
    expect(
      circuitToolResponseSchemas.correct.safeParse({
        ok: false,
        error: { code: "busy", message: "x" },
      }).success,
    ).toBe(false);
    // 分析类工具同样只认规格的七个错误码。
    expect(circuitToolResponseSchemas.compareFirmware.safeParse(failure).success).toBe(true);
    expect(
      circuitToolResponseSchemas.compareFirmware.safeParse({
        ok: false,
        error: { code: "unsupported", message: "x" },
      }).success,
    ).toBe(false);
  });
});
