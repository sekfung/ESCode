import { describe, expect, it } from "vitest";
import { SERVER_REMOTE_PROTOCOL_VERSION, serverRemoteInfoSchema } from "@escode/shared";

const serverInfo = {
  serverId: "remote-server",
  version: "3.12.0",
  protocolVersion: SERVER_REMOTE_PROTOCOL_VERSION,
  authRequired: false,
  workspaces: [],
  capabilities: { desktopContinuous: true, websocketRpc: true },
};

describe("Server 远端能力协商", () => {
  it("解析旧 Server 的连续流与 WebSocket RPC 能力", () => {
    const parsed = serverRemoteInfoSchema.parse(serverInfo);
    expect(parsed).toEqual(serverInfo);
    expect(parsed.protocolVersion).toBe(1);
  });
});
