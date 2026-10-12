import { useMemo } from "react";
import type { CodeViewerSource } from "@/lib/codeViewer.js";
import { toWorkspaceRelativePath } from "@/lib/taskChangeSummary.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { useWorkspaceServices } from "@/hooks/useWorkspaceServices.js";
import { type CircuitPanelFileKind } from "@/circuit/domain/circuitPanelSource.js";
import { CircuitPanel } from "@/circuit/components/CircuitPanel.js";

/**
 * PreviewPane 里的电路入口（规格 §8「在文件树中打开 .kicad_sch / .kicad_pcb / .circuit.json」）。
 * 文件树沿既有的 code-viewer 面板打开文件，这里按扩展名把内容路由到电路面板：
 * PDF / 图片显示「让 Agent 识别」提示，面板不发起模型识别；快照只读展示。
 */

export function CircuitPreviewContent({
  source,
  workspacePath,
  snapshotText,
  snapshotPending,
  fileKind,
}: {
  source: CodeViewerSource;
  workspacePath?: string;
  /** `.circuit.json` 的文本（PreviewPane 已读到的内容，避免二次 IO）。 */
  snapshotText?: string | null;
  snapshotPending?: boolean;
  fileKind: CircuitPanelFileKind;
}) {
  const { intl } = useESCodeIntl();
  const path = source.path;
  const remoteSessionId = source.workspaceRemoteSessionId;
  const workspaceIdentity = source.workspaceIdentity;
  const services = useWorkspaceServices(workspacePath, remoteSessionId, workspaceIdentity);
  // workspace 引用必须稳定：面板的解析 effect 以它作为依赖，每次渲染新建对象会重新解析。
  const workspace = useMemo(
    () =>
      workspacePath === undefined
        ? null
        : {
            workspacePath,
            ...(workspaceIdentity ? { workspaceIdentity } : {}),
            ...(remoteSessionId ? { remoteSessionId } : {}),
          },
    [remoteSessionId, workspaceIdentity, workspacePath],
  );
  const relativePath = useMemo(
    () => (path && workspacePath ? toWorkspaceRelativePath(workspacePath, path) : undefined),
    [path, workspacePath],
  );
  if (fileKind === "snapshot" && snapshotPending)
    return (
      <p className="p-4 text-ui-sm text-foreground-subtle" data-testid="circuit-panel-loading">
        {intl.formatMessage({ id: "circuit.loading" })}
      </p>
    );
  return (
    <CircuitPanel
      services={services}
      workspace={workspace}
      path={relativePath}
      title={source.title}
      snapshotText={snapshotText}
      requestedIrId={source.type === "file" ? source.circuitPanel?.irId : undefined}
    />
  );
}
