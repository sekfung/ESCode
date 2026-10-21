import { afterEach, describe, expect, it, vi } from "vitest";
import { AddressInfo } from "node:net";
import { WebSocket, type RawData } from "ws";
import { Emitter, Event, SocketProtocol, VSBuffer, type ISocket } from "@escode/rpc";
import { ServiceCollection, ISystemService, IESCodeAgentService } from "@escode/services";
import { createHttpServer } from "@escode/server";
import { connectViaProtocol, connectViaWebSocket } from "@escode/client";
import {
  connectServerRemote,
  resolveServerRemoteEndpoints,
} from "../src/host/serverRemoteConnection.js";
import { serverRemoteInfoSchema, type AgentLaneResourceSample } from "@escode/shared";

function wrapNodeWebSocket(socket: WebSocket): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();
  socket.on("message", (raw: RawData) => {
    const data = Array.isArray(raw)
      ? Buffer.concat(raw)
      : Buffer.isBuffer(raw)
        ? raw
        : Buffer.from(raw);
    onData.fire(VSBuffer.wrap(new Uint8Array(data)));
  });
  socket.on("close", () => {
    onClose.fire();
    onEnd.fire();
  });
  socket.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });
  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer) {
      if (socket.readyState === WebSocket.OPEN) socket.send(buffer.buffer);
    },
    end() {
      socket.close();
    },
    drain: async () => {},
    dispose() {
      socket.close();
    },
  };
}

function connectNodeRpc(
  url: string,
  headers: Record<string, string>,
): Promise<{ socket: WebSocket; services: ReturnType<typeof connectViaProtocol> }> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url, { headers });
    socket.once("error", reject);
    socket.once("open", () => {
      resolve({
        socket,
        services: connectViaProtocol(new SocketProtocol(wrapNodeWebSocket(socket))),
      });
    });
  });
}

function rejectedWebSocketStatus(
  url: string,
  headers: Record<string, string> = {},
): Promise<number | undefined> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url, { headers });
    socket.once("unexpected-response", (_request, response) => {
      response.resume();
      resolve(response.statusCode);
    });
    socket.once("open", () => {
      socket.close();
      reject(new Error("WebSocket unexpectedly opened"));
    });
    socket.once("error", () => {});
  });
}

describe("server remote connection", () => {
  const servers: Array<{ close(callback?: (err?: Error) => void): void }> = [];
  const disposables: Array<{ dispose(): void }> = [];

  afterEach(async () => {
    for (const disposable of disposables.splice(0)) {
      disposable.dispose();
    }
    await Promise.all(
      servers.map(
        (server) =>
          new Promise<void>((resolve, reject) => {
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

  it("resolves info and websocket endpoints from an HTTP server URL", () => {
    expect(resolveServerRemoteEndpoints("https://studio.example.com/escode/")).toEqual({
      infoUrl: "https://studio.example.com/escode/api/server-info",
      wsUrl: "wss://studio.example.com/escode/ws",
      hostCapabilityUrl: "https://studio.example.com/escode/api/rpc-host-capability",
      hostWsUrl: "wss://studio.example.com/escode/ws/host",
    });
    expect(resolveServerRemoteEndpoints("ws://127.0.0.1:3030/ws")).toEqual({
      infoUrl: "http://127.0.0.1:3030/api/server-info",
      wsUrl: "ws://127.0.0.1:3030/ws",
      hostCapabilityUrl: "http://127.0.0.1:3030/api/rpc-host-capability",
      hostWsUrl: "ws://127.0.0.1:3030/ws/host",
    });
  });

  it("connects to a running escode server over websocket RPC", async () => {
    const info = vi.fn(async () => ({
      homedir: "/home/dev",
      platform: "linux",
    }));
    const services = new ServiceCollection()
      .register(ISystemService, {
        info,
        listIntegratedTerminalShells: vi.fn(async () => []),
        probeIntranet: vi.fn(async () => ({
          checkedAt: 1,
          isIntranet: false,
          reachedTargetCount: 0,
          requiredSuccessCount: 1,
          results: [],
          strategy: "tcp-connect",
          totalTargets: 0,
        })),
      })
      .register(IESCodeAgentService, {
        helloConversationV4: vi.fn(),
        initializeConversationV4: vi.fn(),
      } as never);
    const server = createHttpServer(services, 0, {
      serverId: "studio",
      workspaces: [{ path: "/home/dev/project" }],
    });
    servers.push(server);
    const address = server.address() as AddressInfo;

    const requestedPaths: string[] = [];
    const connection = await connectServerRemote(
      {
        kind: "server",
        url: `http://127.0.0.1:${address.port}`,
      },
      {
        fetchImpl: async (input, init) => {
          requestedPaths.push(new URL(String(input)).pathname);
          return fetch(input, init);
        },
      },
    );
    disposables.push(connection);

    await expect(connection.services.systemService.info()).resolves.toEqual({
      homedir: "/home/dev",
      platform: "linux",
    });
    expect(connection.serverInfo).toMatchObject({
      serverId: "studio",
      workspaces: [{ path: "/home/dev/project" }],
    });
    await expect(
      connection.services.escodeAgentService.helloConversationV4(),
    ).resolves.toMatchObject({
      clientMode: "desktop-continuous",
      deliveryProfile: "continuous",
    });
    expect(requestedPaths).toEqual(["/api/server-info", "/api/rpc-host-capability"]);
    let browserSocket: WebSocket | undefined;
    const browserServices = await connectViaWebSocket(`ws://127.0.0.1:${address.port}/ws`, {
      onOpenSocket: (socket) => (browserSocket = socket),
    });
    await expect(browserServices.escodeAgentService.helloConversationV4()).resolves.toMatchObject({
      clientMode: "web-remote-replayable",
      deliveryProfile: "replayable",
    });
    browserSocket?.close();

    const legacyHeaderConnection = await connectNodeRpc(`ws://127.0.0.1:${address.port}/ws`, {
      "x-zcode-rpc-client-mode": "desktop-continuous",
    });
    await expect(
      legacyHeaderConnection.services.escodeAgentService.helloConversationV4(),
    ).resolves.toMatchObject({
      clientMode: "web-remote-replayable",
      deliveryProfile: "replayable",
    });
    legacyHeaderConnection.socket.close();
    expect(info).toHaveBeenCalledOnce();
  });

  it("connector 使用 token 贯穿受保护的 capability endpoint 与 /ws/host", async () => {
    const services = new ServiceCollection().register(IESCodeAgentService, {
      helloConversationV4: vi.fn(),
      initializeConversationV4: vi.fn(),
    } as never);
    const server = createHttpServer(services, 0, {
      authRequired: true,
      authToken: "secret-token",
    });
    servers.push(server);
    const address = server.address() as AddressInfo;
    const requestedUrls: URL[] = [];

    const connection = await connectServerRemote(
      {
        kind: "server",
        url: `http://127.0.0.1:${address.port}`,
        token: "secret-token",
      },
      {
        fetchImpl: async (input, init) => {
          requestedUrls.push(new URL(String(input)));
          return fetch(input, init);
        },
      },
    );
    disposables.push(connection);

    await expect(
      connection.services.escodeAgentService.helloConversationV4(),
    ).resolves.toMatchObject({
      clientMode: "desktop-continuous",
      deliveryProfile: "continuous",
    });
    expect(requestedUrls.map((url) => url.pathname)).toEqual([
      "/api/server-info",
      "/api/rpc-host-capability",
    ]);
    expect(requestedUrls.every((url) => url.searchParams.get("token") === "secret-token")).toBe(
      true,
    );
  });

  it("/ws/host 只接受已申请的一次性 capability，重放被拒绝", async () => {
    const services = new ServiceCollection().register(IESCodeAgentService, {
      helloConversationV4: vi.fn(),
      initializeConversationV4: vi.fn(),
    } as never);
    const server = createHttpServer(services, 0);
    servers.push(server);
    const address = server.address() as AddressInfo;
    const httpBase = `http://127.0.0.1:${address.port}`;
    const wsHost = `ws://127.0.0.1:${address.port}/ws/host`;

    expect(await rejectedWebSocketStatus(wsHost)).toBe(401);
    expect(
      await rejectedWebSocketStatus(wsHost, {
        "x-zcode-rpc-host-capability": "forged",
      }),
    ).toBe(401);

    const ticketResponse = await fetch(`${httpBase}/api/rpc-host-capability`, {
      method: "POST",
    });
    expect(ticketResponse.status).toBe(200);
    const ticket = (await ticketResponse.json()) as {
      capability: string;
      expiresAt: number;
    };
    expect(ticket.capability).toEqual(expect.any(String));
    expect(ticket.expiresAt).toBeGreaterThan(Date.now());

    const trusted = await connectNodeRpc(wsHost, {
      "x-zcode-rpc-host-capability": ticket.capability,
    });
    await expect(trusted.services.escodeAgentService.helloConversationV4()).resolves.toMatchObject({
      clientMode: "desktop-continuous",
      deliveryProfile: "continuous",
    });
    expect(
      await rejectedWebSocketStatus(wsHost, {
        "x-zcode-rpc-host-capability": ticket.capability,
      }),
    ).toBe(401);
    trusted.socket.close();
  });
});
