import { useCallback, useState } from "react";
import { CircuitBoardIcon } from "lucide-react";
import type { CircuitReference } from "@escode/shared/circuit";
import type { CodeViewerSource } from "@/lib/codeViewer.js";
import { Button } from "@/components/ui/button.js";
import { Spinner } from "@/components/ui/spinner.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { usePlatform } from "@/hooks/usePlatform.js";
import { useOptionalServices } from "@/hooks/useServices.js";
import { logger } from "@/logger.js";
import { describeCircuitError } from "@/circuit/domain/circuitPanelSource.js";

/**
 * `::circuit` 引用卡片（规格 §8、§3.5）。
 * 只在 Desktop（有能力位与 CircuitService）渲染：点击经服务把 irId 解析成源文件路径，再打开右侧电路面板。
 * Web / 手机没有 CircuitService，`::circuit` 不渲染任何内容——不给一个点不动的降级占位。
 */

export interface CircuitCardWorkspace {
  workspacePath?: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}

export function CircuitReferenceCard({
  reference,
  workspace,
  onOpenCodeViewer,
}: {
  reference: CircuitReference;
  workspace: CircuitCardWorkspace;
  onOpenCodeViewer?: (source: CodeViewerSource) => void;
}) {
  const { intl } = useESCodeIntl();
  const platform = usePlatform();
  const services = useOptionalServices();
  const circuitService = services?.circuitService;
  // Web / 手机上服务代理仍然存在（通道未注册），平台能力是这里唯一可靠的判据（§3.5）。
  const desktopCircuitAvailable = platform.supportsCircuit === true;
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const title = reference.title?.trim() || reference.irId;

  const openPanel = useCallback(async () => {
    if (!desktopCircuitAvailable || !circuitService || !workspace.workspacePath) return;
    setOpening(true);
    setError(null);
    try {
      const view = await circuitService.get({
        workspacePath: workspace.workspacePath,
        ...(workspace.workspaceIdentity ? { workspaceIdentity: workspace.workspaceIdentity } : {}),
        ...(workspace.remoteSessionId ? { remoteSessionId: workspace.remoteSessionId } : {}),
        irId: reference.irId,
      });
      onOpenCodeViewer?.({
        type: "file",
        title: view.summary.source.path,
        path: view.summary.source.path,
        circuitPanel: { irId: reference.irId },
        workspacePath: workspace.workspacePath,
        ...(workspace.workspaceIdentity ? { workspaceIdentity: workspace.workspaceIdentity } : {}),
        ...(workspace.remoteSessionId
          ? { workspaceRemoteSessionId: workspace.remoteSessionId }
          : {}),
      });
    } catch (openError) {
      const described = describeCircuitError(openError);
      logger.warn("[circuit] reference card open failed", described);
      setError(
        intl.formatMessage(
          { id: `circuit.error.${described.code ?? "io_error"}` },
          { message: described.message },
        ),
      );
    } finally {
      setOpening(false);
    }
  }, [circuitService, desktopCircuitAvailable, intl, onOpenCodeViewer, reference.irId, workspace]);

  if (!desktopCircuitAvailable || !circuitService) return null;

  return (
    <section
      className="my-3 flex w-full items-center gap-3 rounded-xl border border-card-border bg-card px-3 py-2"
      data-testid="circuit-card"
      data-circuit-card-mode="desktop"
    >
      <CircuitBoardIcon className="size-5 shrink-0 text-foreground-subtle" />
      <div className="min-w-0 flex-1">
        <p className="truncate text-ui-base">{title}</p>
        {reference.title ? (
          <p className="truncate text-ui-sm text-foreground-subtle">{reference.irId}</p>
        ) : null}
        {error ? (
          <p className="break-words text-ui-xs text-destructive" role="alert">
            {error}
          </p>
        ) : null}
      </div>
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={opening || !onOpenCodeViewer}
        onClick={() => void openPanel()}
        data-testid="circuit-card-open"
      >
        {opening ? <Spinner className="size-3.5" /> : null}
        {intl.formatMessage({ id: "circuit.card.open" })}
      </Button>
    </section>
  );
}
