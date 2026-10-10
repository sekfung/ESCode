import assert from "node:assert/strict";
import test from "node:test";
import { zcodeSerialCancelMethod, zcodeSerialMethods } from "@zcode/shared/serial";
import { SerialError } from "../src/serial/serial.js";
import type { SerialAgentBridge } from "../src/serial/serialAgentBridge.js";
import {
  SERIAL_TOOL_ERROR_RPC_CODE,
  routeSerialAgentRequest,
} from "../src/serial/serialAgentRequestRouter.js";

function createResponder() {
  const responses: Array<{ id: unknown; result?: unknown; error?: unknown }> = [];
  return {
    responses,
    responder: {
      respond: async (id: string | number, result: unknown) => {
        responses.push({ id, result });
      },
      respondError: async (
        id: string | number,
        error: { code: number; message: string; data?: unknown },
      ) => {
        responses.push({ id, error });
      },
    },
  };
}

const baseParams = {
  requestId: "req-1",
  sessionId: "session-a",
  workspaceKey: "C:/work",
  workspacePath: "C:/work",
};

const flush = () => new Promise((resolve) => setImmediate(resolve));

test("非串口方法不处理，交给后续分支", () => {
  const { responder } = createResponder();
  const handled = routeSerialAgentRequest({
    request: { id: 1, method: "interaction/browserList", params: {} },
    bridge: undefined,
    responder,
  });
  assert.equal(handled, false);
});

test("参数非法时返回 -32602", async () => {
  const { responder, responses } = createResponder();
  const handled = routeSerialAgentRequest({
    request: {
      id: 2,
      method: zcodeSerialMethods.read,
      params: { ...baseParams, args: { maxBytes: -1 } },
    },
    bridge: undefined,
    responder,
  });
  assert.equal(handled, true);
  await flush();
  const error = responses[0]?.error as { code: number } | undefined;
  assert.equal(error?.code, -32602);
});

test("Host 未装配 bridge 时返回 unavailable 工具错误", async () => {
  const { responder, responses } = createResponder();
  routeSerialAgentRequest({
    request: { id: 3, method: zcodeSerialMethods.list, params: { ...baseParams, args: {} } },
    bridge: undefined,
    responder,
  });
  await flush();
  assert.deepEqual(responses[0]?.error, {
    code: SERIAL_TOOL_ERROR_RPC_CODE,
    message: "Serial port tools are not available here",
    data: { code: "unavailable" },
  });
});

test("bridge 结果原样返回，SerialError 映射为带 code 的工具错误", async () => {
  const { responder, responses } = createResponder();
  const bridge: SerialAgentBridge = {
    handle: async (op) => {
      if (op === "write") throw new SerialError("notOpen", "Serial port is not open");
      return { ports: [], status: { state: "closed" } } as never;
    },
    cancel: () => {},
  };
  routeSerialAgentRequest({
    request: { id: 4, method: zcodeSerialMethods.list, params: { ...baseParams, args: {} } },
    bridge,
    responder,
  });
  routeSerialAgentRequest({
    request: {
      id: 5,
      method: zcodeSerialMethods.write,
      params: { ...baseParams, args: { data: "AT" } },
    },
    bridge,
    responder,
  });
  await flush();
  await flush();
  assert.deepEqual(responses.find((item) => item.id === 4)?.result, {
    ports: [],
    status: { state: "closed" },
  });
  assert.deepEqual(responses.find((item) => item.id === 5)?.error, {
    code: SERIAL_TOOL_ERROR_RPC_CODE,
    message: "Serial port is not open",
    data: { code: "notOpen" },
  });
});

test("取消方法转给 bridge.cancel 并立即应答", async () => {
  const { responder, responses } = createResponder();
  const cancelled: unknown[] = [];
  routeSerialAgentRequest({
    request: {
      id: 6,
      method: zcodeSerialCancelMethod,
      params: { sessionId: "session-a", targetRequestId: "req-1" },
    },
    bridge: {
      handle: async () => ({}) as never,
      cancel: (params) => {
        cancelled.push(params);
      },
    },
    responder,
  });
  await flush();
  assert.deepEqual(cancelled, [{ sessionId: "session-a", targetRequestId: "req-1" }]);
  assert.deepEqual(responses[0], { id: 6, result: {} });
});
