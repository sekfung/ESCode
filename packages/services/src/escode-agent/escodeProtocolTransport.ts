import type { Event, IDisposable } from "@escode/rpc";
import type { ESCodeProtocolMessage } from "@escode/shared";

export type ESCodeProtocolTransportKind = "stdio" | "websocket" | "memory";

export interface ESCodeProtocolTransportClosedEvent {
  code?: number | null;
  signal?: NodeJS.Signals | null;
  reason?: string;
}

export interface ESCodeProtocolTransport extends IDisposable {
  readonly kind: ESCodeProtocolTransportKind;
  readonly onMessage: Event<ESCodeProtocolMessage>;
  readonly onClose: Event<ESCodeProtocolTransportClosedEvent>;
  send(message: ESCodeProtocolMessage): Promise<void>;
  disposeAndWait?(): Promise<void>;
}
