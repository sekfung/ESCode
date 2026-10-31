import { ChannelClient, ChannelServer, createQueuePair, type IChannel } from "@escode/rpc";
import { createServiceDescriptor, ServiceCollection } from "@escode/services";
import { describe, expect, it, vi } from "vitest";
import { exposeServicesAndReady } from "../src/stdio.js";

describe("stdio Initialize capabilities", () => {
  it("broadcasts channels registered before ready and serves calls on them", async () => {
    const [serverProtocol, clientProtocol] = createQueuePair();
    // 与 createStdioServer 一致：先 defer，注册服务后再广播。
    const server = new ChannelServer(serverProtocol, "stdio-test", 1000, true);
    const client = new ChannelClient(clientProtocol);
    const services = new ServiceCollection();
    const descriptor = createServiceDescriptor<{ ping(): Promise<string> }>("file");
    services.register(descriptor, { ping: async () => "pong" });

    exposeServicesAndReady(server, services);

    await vi.waitFor(() => {
      expect(client.hasChannel("file")).toBe(true);
    });
    // 未注册的 channel 不应被广播（远端 scope 用它决定转发入口）。
    expect(client.hasChannel("window-controller")).toBe(false);
    const channel = client.getChannel<IChannel>("file");
    await expect(channel.call("ping")).resolves.toBe("pong");

    server.dispose();
    client.dispose();
  });
});
