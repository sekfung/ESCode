/**
 * ESCode Agent Slash Commands 便捷 hook
 *
 * 返回当前 workspace 下 Agent 广播的可用 slash commands 列表。
 */
import { useESCodeSessionStore, selectWorkspaceESCodeState } from "../store/escodeSessionStore.js";

export function useSlashCommands(workspacePath: string, workspaceIdentity?: string) {
  return useESCodeSessionStore(
    (state) => selectWorkspaceESCodeState(state, workspacePath, workspaceIdentity).slashCommands,
  );
}
