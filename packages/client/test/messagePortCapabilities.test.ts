import { describe, expect, it } from "vitest";
import {
  ChannelServer,
  Event,
  MessagePortProtocol,
  type MessagePortLike,
  type IChannel,
} from "@escode/rpc";
import { createMessagePortServiceConnection } from "../src/messageport.js";

// 渲染器真实路径：MessagePort ↔ ChannelServer，`createMessagePortServiceConnection().ready`
// 必须在 Initialize（能力广播）之后才构建访问器。这里用内存端口对复现该时序，不起 Electron。

type Listener = (event: { data: unknown }) => void;

function createPortPair(): [MessagePortLike, MessagePortLike] {
  const listeners: [Set<Listener>, Set<Listener>] = [new Set(), new Set()];
  const make = (self: 0 | 1): MessagePortLike => ({
    addEventListener: (_type, listener) => {
      listeners[self].add(listener as Listener);
    },
    removeEventListener: (_type, listener) => {
      listeners[self].delete(listener as Listener);
    },
    postMessage: (data) => {
      const target = listeners[self === 0 ? 1 : 0];
      queueMicrotask(() => {
        for (const listener of target) listener({ data });
      });
    },
    start: () => undefined,
    close: () => listeners[self].clear(),
  });
  return [make(0), make(1)];
}

function startHost(port: MessagePortLike, channels: string[]) {
  const server = new ChannelServer(
    new MessagePortProtocol(port),
    "test-host",
    1000,
    true,
  );
  for (const name of channels) {
    server.registerChannel(name, {
      call: async (_ctx: string, command: string) => ({
        channel: name,
        command,
      }),
      listen: () => Event.None,
    });
  }
  server.ready();
  return server;
}

const SNAPSHOT = {
  generation: 1,
  profiles: [],
  servers: [],
  workspaceHistory: [],
  states: [],
  failedPathCount: 0,
  abandonedPathCount: 0,
  migrationCounts: [],
};

function startSourceServerHost(port: MessagePortLike) {
  const server = new ChannelServer(
    new MessagePortProtocol(port),
    "test-host",
    1000,
    true,
  );
  server.registerChannel("source-server-management", {
    call: async (_ctx: string, command: string) =>
      command === "getSnapshot" ? SNAPSHOT : [],
    listen: () => Event.None,
  } satisfies IChannel);
  server.ready();
  return server;
}

describe("message port source server capability", () => {
  it("binds the management service only after the host advertises the channel", async () => {
    const [clientPort, hostPort] = createPortPair();
    const server = startSourceServerHost(hostPort);
    const connection = createMessagePortServiceConnection(clientPort);

    const services = await connection.ready;
    const management = services.sourceServerManagementService;
    expect(management).toBeDefined();
    expect(await management!.getSnapshot()).toEqual(SNAPSHOT);

    connection.dispose();
    server.dispose();
  });

  it("leaves the management service unbound when the host does not advertise it", async () => {
    const [clientPort, hostPort] = createPortPair();
    const server = startHost(hostPort, ["setting"]);
    const connection = createMessagePortServiceConnection(clientPort);

    const services = await connection.ready;
    expect(services.sourceServerManagementService).toBeUndefined();

    connection.dispose();
    server.dispose();
  });

  it("resolves ready without hanging when the port is disposed before Initialize", async () => {
    const [clientPort] = createPortPair();
    const connection = createMessagePortServiceConnection(clientPort);

    connection.dispose(new Error("superseded"));
    const services = await connection.ready;

    // 握手前释放：按“能力未知”处理，不绑定可选服务，也不会永久挂起。
    expect(services.sourceServerManagementService).toBeUndefined();
  });
});
