import { describe, expect, it } from "vitest";
import {
  buildRemoteWorkspaceConnectResultTelemetry,
  classifyRemoteUsageError,
} from "../src/remoteUsageTelemetry.js";

describe("remote usage telemetry", () => {
  it("构造远程工作区连接终态事件且不携带目标身份信息", () => {
    const event = buildRemoteWorkspaceConnectResultTelemetry({
      result: "success",
      remoteKind: "ssh",
      connectTrigger: "reconnect",
    });

    expect(event).toEqual({
      elementName: "remote_workspace_connect_result",
      eventRegion: "remote_workspace",
      eventType: "result",
      eventExtraDetail: {
        result: "success",
        remote_kind: "ssh",
        connect_trigger: "reconnect",
        error_category: "",
      },
    });
    expect(event.eventExtraDetail).not.toHaveProperty("workspacePath");
    expect(event.eventExtraDetail).not.toHaveProperty("workspaceIdentity");
    expect(event.eventExtraDetail).not.toHaveProperty("remoteSessionId");
  });

  it.each([
    [Object.assign(new Error("password rejected"), { code: "AUTH_FAILED" }), "auth"],
    [Object.assign(new Error("asset install failed"), { code: "DEPLOY_FAILED" }), "deploy"],
    [Object.assign(new Error("spawn host failed"), { code: "HOST_START_FAILED" }), "host_start"],
    [
      Object.assign(new Error("secret workspace missing"), { code: "REMOTE_SESSION_MISSING" }),
      "attach",
    ],
    [Object.assign(new Error("desktop unavailable"), { code: "DESKTOP_HOST_MISSING" }), "attach"],
    [Object.assign(new Error("relay websocket closed"), { code: "RELAY_CLOSED" }), "relay"],
    [Object.assign(new Error("ssh timeout"), { code: "ETIMEDOUT" }), "connect"],
    [new Error("包含私密目标但无法分类"), "unknown"],
  ] as const)("只把错误映射为低基数枚举 %#", (error, expected) => {
    expect(classifyRemoteUsageError(error)).toBe(expected);
  });
});
