import { randomUUID } from "node:crypto";
import {
  Emitter,
  VSBuffer,
  SocketProtocol,
  ChannelServer,
  ChannelClient,
  type ISocket,
} from "@escode/rpc";
import {
  IESCodeAgentService,
  createESCodeAgentConnectionScope,
  type ServiceCollection,
} from "@escode/services";

/**
 * Wrap process.stdin/stdout as an ISocket for RPC communication.
 * In stdio mode, stdout is reserved exclusively for RPC data.
 * All logging must go through stderr.
 */
export function wrapStdio(): ISocket {
  const onData = new Emitter<VSBuffer>();
  const onClose = new Emitter<void>();
  const onEnd = new Emitter<void>();

  process.stdin.on("data", (chunk: Buffer) => {
    onData.fire(VSBuffer.wrap(new Uint8Array(chunk)));
  });
  process.stdin.on("end", () => {
    onClose.fire();
    onEnd.fire();
  });
  process.stdin.on("error", () => {
    onClose.fire();
    onEnd.fire();
  });

  return {
    onData: onData.event,
    onClose: onClose.event,
    onEnd: onEnd.event,
    write(buffer: VSBuffer) {
      process.stdout.write(Buffer.from(buffer.buffer));
    },
    end() {
      process.stdout.end();
    },
    drain() {
      return new Promise<void>((resolve) => {
        if (process.stdout.writableNeedDrain) {
          process.stdout.once("drain", resolve);
        } else {
          resolve();
        }
      });
    },
    dispose() {
      process.stdin.destroy();
    },
  };
}

export function createStdioServer(
  services: ServiceCollection,
  onReverseClientReady?: (client: ChannelClient) => void,
) {
  const socket = wrapStdio();
  const protocol = new SocketProtocol(socket);
  const channelServer = new ChannelServer(protocol, "stdio", 1000, true);
  const reverseClient = onReverseClientReady ? new ChannelClient(protocol) : undefined;
  if (reverseClient) onReverseClientReady?.(reverseClient);
  const agentService = services.getOptional(IESCodeAgentService);
  const connectionScope = agentService
    ? createESCodeAgentConnectionScope(agentService, {
        connectionId: `server-stdio-${randomUUID()}`,
        clientMode: "desktop-continuous",
        role: "trusted-host-relay",
      })
    : undefined;
  // 修复原因：ChannelServer 构造期默认立即发 Initialize，此时 exposeOnChannelServer 还没注册任何
  // channel，能力广播里的 channels 恒为空；Rust Host 只转发远端广播过的 channel，远端 scope 因此
  // 只剩 window-controller，file.readdir 等调用报 NotImplemented。与 Desktop Host 一致，先注册再广播。
  exposeServicesAndReady(
    channelServer,
    services,
    connectionScope
      ? new Map([[IESCodeAgentService.channelName, connectionScope.service]])
      : new Map(),
  );
  let stopPromise: Promise<void> | undefined;
  const stop = (): Promise<void> => {
    if (stopPromise) {
      return stopPromise;
    }

    // 先同步摘掉 protocol listener，保证从这一刻起不再接收新的 service RPC；
    // connection scope 的异步退订完成后再关闭底层 stdio。
    channelServer.dispose();
    reverseClient?.dispose();
    stopPromise = (async () => {
      try {
        await connectionScope?.dispose();
      } finally {
        socket.dispose();
      }
    })();
    return stopPromise;
  };
  socket.onClose(() => {
    void stop();
  });
  return { stop };
}

/** 先把全部服务注册到 channel server，再广播 Initialize 能力集（defer-init 的收口点）。 */
export function exposeServicesAndReady(
  channelServer: ChannelServer,
  services: ServiceCollection,
  overrides: ReadonlyMap<string, unknown> = new Map(),
): void {
  services.exposeOnChannelServer(channelServer, overrides);
  channelServer.ready();
}
