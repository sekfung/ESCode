import { MessagePortProtocol, ChannelClient } from "@escode/rpc";
import type { IServiceAccessor } from "@escode/services";
import { RemoteServiceAccess } from "./remoteServiceAccess.js";
import { isRendererProductionBuild } from "./rendererLoggingEnv.js";

function logMessagePortDebug(message: string): void {
  // 生产构建下 renderer 连接日志不输出，避免窗口启动和重连路径产生同步 console 成本。
  if (isRendererProductionBuild()) {
    return;
  }
  console.log(message);
}

export interface MessagePortServiceConnection {
  /**
   * 服务访问器：Initialize（含 channel 能力广播）之后才构建，可选服务按能力绑定。
   * 端口在握手前被释放时仍会 resolve（能力未知），调用自行以连接错误失败。
   */
  ready: Promise<IServiceAccessor>;
  /** 可在握手完成前调用：关闭协议并让 `ready` 立刻 resolve。 */
  dispose: (reason?: Error) => void;
}

/**
 * 创建一个带明确生命周期的 MessagePort service connection。
 *
 * scoped remote session 换代时必须同时释放 ChannelClient 和底层 port，
 * 否则旧 attachment 上的挂起 RPC 无法 settle，并会继续占用上层去重状态。
 */
export function createMessagePortServiceConnection(
  port: MessagePort,
): MessagePortServiceConnection {
  logMessagePortDebug("[messageport] creating protocol and client...");
  const protocol = new MessagePortProtocol(port);
  const client = new ChannelClient(protocol);
  client.onDidInitialize(() => {
    logMessagePortDebug("[messageport] ChannelClient received Initialize from server");
  });
  logMessagePortDebug("[messageport] client created, waiting for Initialize...");

  // Initialize 携带 channel 能力广播；等服务端握手完成后再建服务访问器，
  // 否则可选服务（如 `source-server-management`）会在能力未知时被判为不可用。
  // 端口在握手前被释放时 `waitUntilInitialized` 会拒绝，这里退回“能力未知”语义：
  // 访问器照旧构建（可选服务不绑定），后续调用以 ConnectionClosed 失败——与原同步构造一致。
  const ready = client
    .waitUntilInitialized()
    .catch(() => undefined)
    .then(() => new RemoteServiceAccess(client));
  let disposed = false;
  return {
    ready,
    dispose: (reason?: Error) => {
      if (disposed) {
        return;
      }
      disposed = true;
      client.dispose(reason);
      protocol.disconnect();
    },
  };
}

/**
 * 通过 MessagePort 连接服务。
 *
 * Desktop 模式下，utilityProcess（或 main 进程的远程代理）通过 MessagePort
 * 暴露 ChannelServer，renderer 用此函数建立 ChannelClient 连接。
 */
export async function connectViaMessagePort(port: MessagePort): Promise<IServiceAccessor> {
  return await createMessagePortServiceConnection(port).ready;
}
