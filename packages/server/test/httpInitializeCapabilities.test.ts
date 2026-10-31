import { randomUUID } from "node:crypto";
import type { Server } from "node:http";
import { AddressInfo } from "node:net";
import { afterEach, describe, expect, it } from "vitest";
import WebSocket from "ws";
import { ChannelClient, Emitter, SocketProtocol, VSBuffer } from "@escode/rpc";
import {
  IProviderProvisioningTargetService,
  ServiceCollection,
  type IProviderProvisioningTargetService as ProviderProvisioningTargetService,
} from "@escode/services";
import { ESCODE_RPC_HOST_CAPABILITY_HEADER } from "@escode/shared";
import { createHttpServer } from "../src/http.js";

interface ProbeClient {
  client: ChannelClient;
  dispose(): void;
}

describe("http Initialize capabilities", () => {
  const servers: Server[] = [];

  afterEach(async () => {
    await Promise.all(
      servers.map(
        (server) =>
          new Promise<void>((resolve, reject) => {
            // WS 长连接不会因为 client.dispose() 立刻释放，显式断开后再 close，
            // 否则 afterEach 会一直等到 hook 超时。
            server.closeAllConnections();
            server.close((error?: Error) => {
              if (error) {
                reject(error);
                return;
              }
              resolve();
            });
          }),
      ),
    );
    servers.length = 0;
  });

  it("broadcasts channels registered before ready for both ws modes", async () => {
    const token = `token-${randomUUID()}`;
    const services = new ServiceCollection();
    services.register(IProviderProvisioningTargetService, {
      apply: async () => "trusted-ok",
    } as unknown as ProviderProvisioningTargetService);
    const server = createHttpServer(services, 0, { authToken: token });
    servers.push(server);
    const address = server.address() as AddressInfo;
    const base = `http://127.0.0.1:${address.port}`;

    const capabilityResponse = await fetch(
      `${base}/api/rpc-host-capability?token=${encodeURIComponent(token)}`,
      { method: "POST" },
    );
    const { capability } = (await capabilityResponse.json()) as { capability: string };

    const trusted = await connect(address.port, "/ws/host", token, {
      [ESCODE_RPC_HOST_CAPABILITY_HEADER]: capability,
    });
    try {
      // 修复前这里恒为 false：Initialize 在 exposeOnChannelServer 之前就广播了空能力集。
      expect(trusted.client.hasChannel(IProviderProvisioningTargetService.channelName)).toBe(true);
      await expect(
        trusted.client
          .getChannel<{ call<T>(command: string, arg?: unknown): Promise<T> }>(
            IProviderProvisioningTargetService.channelName,
          )
          .call("apply", {}),
      ).resolves.toBe("trusted-ok");
    } finally {
      trusted.dispose();
    }

    const web = await connect(address.port, "/ws", token);
    try {
      expect(web.client.hasChannel(IProviderProvisioningTargetService.channelName)).toBe(true);
      // 非受信 Host 只拿到显式拒绝的 override，不能执行 provisioning apply。
      await expect(
        web.client
          .getChannel<{ call<T>(command: string, arg?: unknown): Promise<T> }>(
            IProviderProvisioningTargetService.channelName,
          )
          .call("apply", {}),
      ).rejects.toThrow(/仅支持受信 Desktop Host/);
    } finally {
      web.dispose();
    }
  });
});

async function connect(
  port: number,
  path: string,
  token: string,
  headers: Record<string, string> = {},
): Promise<ProbeClient> {
  const separator = path.includes("?") ? "&" : "?";
  const ws = new WebSocket(
    `ws://127.0.0.1:${port}${path}${separator}token=${encodeURIComponent(token)}`,
    { headers },
  );
  // Initialize 可能在 open 之后立刻到达；Emitter 没有监听者时会丢弃事件，
  // 因此先构造 ChannelClient（它在构造期订阅 onData），再等待 open。
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  ws.on("message", (raw: Buffer) => onData.fire(VSBuffer.wrap(new Uint8Array(raw))));
  ws.on("close", () => onClose.fire());
  const client = new ChannelClient(
    new SocketProtocol({
      onData: onData.event,
      onClose: onClose.event,
      onEnd: onClose.event,
      write: (buffer) => ws.send(buffer.buffer),
      end: () => ws.close(),
      drain: async () => undefined,
      dispose: () => ws.close(),
    }),
  );
  await new Promise<void>((resolve, reject) => {
    ws.once("open", () => resolve());
    ws.once("error", reject);
  });
  await client.waitUntilInitialized();
  return {
    client,
    dispose: () => {
      client.dispose();
      ws.close();
    },
  };
}
