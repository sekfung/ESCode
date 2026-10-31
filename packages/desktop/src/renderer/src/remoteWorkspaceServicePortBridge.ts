import {
  InternalChannels,
  scopedServicePortPayloadSchema,
  type RemoteTarget,
  type WorkspacePurpose,
} from "@escode/shared";

export interface RemoteWorkspaceServicePortRegistration {
  attachmentId: string;
  port: MessagePort;
  sessionId: string;
  target: RemoteTarget;
  sourceServerId?: string;
  workspacePath: string;
  workspaceIdentity: string;
  workspacePurpose?: WorkspacePurpose;
}

export function parseRemoteWorkspaceServicePortMessage(
  event: MessageEvent,
): RemoteWorkspaceServicePortRegistration | null {
  if (
    typeof event.data !== "object" ||
    event.data === null ||
    !("type" in event.data) ||
    event.data.type !== InternalChannels.ScopedServicePort
  ) {
    return null;
  }
  const port = event.ports[0];
  const { type: _type, ...payload } = event.data as Record<string, unknown>;
  // scope 与 Main 共用 shared schema：sourceServerId 可缺省（无 Source 身份），
  // workspacePath / workspaceIdentity 必须存在，否则无法完成 session 绑定与项目导航。
  const parsed = scopedServicePortPayloadSchema.safeParse(payload);
  if (!port || !parsed.success) return null;
  return { ...parsed.data, port };
}

export function notifyRemoteWorkspaceServicePortReady(
  payload: Pick<RemoteWorkspaceServicePortRegistration, "attachmentId" | "sessionId">,
  postMessage: (message: unknown, targetOrigin: string) => void = window.postMessage.bind(window),
): void {
  postMessage(
    {
      type: InternalChannels.ScopedServicePortReady,
      attachmentId: payload.attachmentId,
      sessionId: payload.sessionId,
    },
    "*",
  );
}
