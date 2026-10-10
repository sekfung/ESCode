import type { ESCodeSessionStateSnapshot } from "@escode/shared";
import type {
  ESCodeSessionWorkspaceTarget,
  ESCodeTaskTarget,
} from "#src/escode-session/escodeSession.js";

function getWorkspaceKey(target: ESCodeSessionWorkspaceTarget): string {
  return target.workspaceIdentity?.trim() || target.workspacePath;
}

function getSessionScopedKey(target: ESCodeTaskTarget): string {
  return `${getWorkspaceKey(target)}\0${target.sessionId}`;
}

export function createESCodeDeferredDraftRegistry() {
  const sessionKeys = new Set<string>();

  return {
    remember(params: ESCodeSessionWorkspaceTarget, snapshot: ESCodeSessionStateSnapshot): void {
      sessionKeys.add(
        getSessionScopedKey({
          workspacePath: snapshot.session.workspace.workspacePath,
          workspaceIdentity:
            snapshot.session.workspace.workspaceIdentity ?? params.workspaceIdentity,
          sessionId: snapshot.session.sessionId,
        }),
      );
    },

    has(target: ESCodeTaskTarget): boolean {
      return sessionKeys.has(getSessionScopedKey(target));
    },

    forget(target: ESCodeTaskTarget): void {
      sessionKeys.delete(getSessionScopedKey(target));
    },
  };
}
