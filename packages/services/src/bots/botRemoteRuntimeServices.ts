import {
  ChannelClient,
  MessagePortProtocol,
  ProxyChannel,
  type MessagePortLike,
  type MessagePortPayload,
} from "@escode/rpc";
import {
  IESCodeTaskService,
  type IESCodeTaskService as IESCodeTaskServiceShape,
} from "#src/session/escodeTaskService.js";
import {
  IESCodeAgentService,
  type IESCodeAgentService as IESCodeAgentServiceShape,
} from "#src/escode-agent/escodeAgent.js";
import {
  IESCodeSessionService,
  type IESCodeSessionService as IESCodeSessionServiceShape,
} from "#src/escode-session/escodeSession.js";
import {
  IModelSelectionService,
  type IModelSelectionService as IModelSelectionServiceShape,
} from "#src/model-provider/providerFacadeServices.js";

interface PortLike {
  on?(event: "message", listener: (event: { data: MessagePortPayload }) => void): void;
  off?(event: "message", listener: (event: { data: MessagePortPayload }) => void): void;
  addEventListener?(
    event: "message",
    listener: (event: { data: MessagePortPayload }) => void,
  ): void;
  removeEventListener?(
    event: "message",
    listener: (event: { data: MessagePortPayload }) => void,
  ): void;
  postMessage(message: MessagePortPayload): void;
  start?(): void;
  close?(): void;
}

function toMessagePortLike(port: PortLike): MessagePortLike {
  return {
    addEventListener(type, listener) {
      if (port.addEventListener) {
        port.addEventListener(type, listener);
        return;
      }
      port.on?.(type, listener);
    },
    removeEventListener(type, listener) {
      if (port.removeEventListener) {
        port.removeEventListener(type, listener);
        return;
      }
      port.off?.(type, listener);
    },
    postMessage(data) {
      port.postMessage(data);
    },
    start() {
      port.start?.();
    },
    close() {
      port.close?.();
    },
  };
}

export interface RemoteBotWorkspaceRuntimeServices {
  escodeAgentService: IESCodeAgentServiceShape;
  escodeTaskService: IESCodeTaskServiceShape;
  escodeSessionService: IESCodeSessionServiceShape;
  modelSelectionService: IModelSelectionServiceShape;
}

export function createRemoteRuntimeServicesFromPort(
  port: unknown,
): RemoteBotWorkspaceRuntimeServices {
  const protocol = new MessagePortProtocol(toMessagePortLike(port as PortLike));
  const client = new ChannelClient(protocol);
  return {
    escodeAgentService: ProxyChannel.toService<IESCodeAgentServiceShape>(
      client.getChannel(IESCodeAgentService.channelName),
    ),
    escodeTaskService: ProxyChannel.toService<IESCodeTaskServiceShape>(
      client.getChannel(IESCodeTaskService.channelName),
    ),
    escodeSessionService: ProxyChannel.toService<IESCodeSessionServiceShape>(
      client.getChannel(IESCodeSessionService.channelName),
    ),
    modelSelectionService: ProxyChannel.toService<IModelSelectionServiceShape>(
      client.getChannel(IModelSelectionService.channelName),
    ),
  };
}
