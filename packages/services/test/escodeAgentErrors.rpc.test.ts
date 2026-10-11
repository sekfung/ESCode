import { ChannelClient, ChannelServer, createQueuePair, ProxyChannel } from "@escode/rpc";
import { describe, expect, it } from "vitest";
import { IESCodeAgentService } from "#src/escode-agent/escodeAgent.js";
import {
  ESCODE_AGENT_MCP_STATUS_MODE_UNSUPPORTED_ERROR_CODE,
  ESCodeAgentMcpStatusModeUnsupportedError,
} from "#src/escode-agent/escodeAgentErrors.js";

describe("ESCode Agent RPC errors", () => {
  it("preserves the MCP status-only unsupported code across ChannelClient RPC", async () => {
    const [serverProtocol, clientProtocol] = createQueuePair();
    const server = new ChannelServer(serverProtocol, "test");
    const client = new ChannelClient(clientProtocol);
    server.registerChannel(
      IESCodeAgentService.channelName,
      ProxyChannel.fromService({
        async listMcpServerStatuses() {
          throw new ESCodeAgentMcpStatusModeUnsupportedError();
        },
      }),
    );
    const service = ProxyChannel.toService<IESCodeAgentService>(
      client.getChannel(IESCodeAgentService.channelName),
    );

    try {
      await expect(
        service.listMcpServerStatuses({
          mcpServers: [],
          mode: "status",
          workspacePath: "/tmp/workspace",
        }),
      ).rejects.toMatchObject({
        code: ESCODE_AGENT_MCP_STATUS_MODE_UNSUPPORTED_ERROR_CODE,
        name: "ESCodeAgentMcpStatusModeUnsupportedError",
      });
    } finally {
      client.dispose();
      server.dispose();
    }
  });
});
