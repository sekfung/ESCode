import { readV4ComposerDraft, V4_DRAFT_SCOPE_ROOT } from "@/v4/composer/composerDraftStore.js";

interface ComposerDraftWorkspaceTransferRequest {
  sourceWorkspacePath: string;
  sourceWorkspaceIdentity?: string;
  targetWorkspacePath: string;
  targetWorkspaceIdentity?: string;
}

let pendingRequest: ComposerDraftWorkspaceTransferRequest | null = null;

function getWorkspaceKey(path: string, identity?: string): string {
  return identity?.trim() || path;
}

export function requestV4ComposerDraftWorkspaceTransfer(
  request: ComposerDraftWorkspaceTransferRequest,
): void {
  pendingRequest = request;
}

export function consumeV4ComposerDraftWorkspaceTransferRequest(
  transition: ComposerDraftWorkspaceTransferRequest,
): boolean {
  if (!pendingRequest) {
    return false;
  }
  const matches =
    getWorkspaceKey(pendingRequest.sourceWorkspacePath, pendingRequest.sourceWorkspaceIdentity) ===
      getWorkspaceKey(transition.sourceWorkspacePath, transition.sourceWorkspaceIdentity) &&
    getWorkspaceKey(pendingRequest.targetWorkspacePath, pendingRequest.targetWorkspaceIdentity) ===
      getWorkspaceKey(transition.targetWorkspacePath, transition.targetWorkspaceIdentity);
  if (matches) {
    pendingRequest = null;
  }
  return matches;
}

export function clearV4ComposerDraftWorkspaceTransferRequest(): void {
  pendingRequest = null;
}

/** 把当前根草稿作为创建任务时的初始内容交给新任务；没有草稿返回 null。 */
export function captureV4ComposerDraftHandoff(
  workspacePath: string,
  workspaceIdentity?: string,
  _clientNamespace?: string,
) {
  return readV4ComposerDraft(workspacePath, workspaceIdentity, V4_DRAFT_SCOPE_ROOT);
}
