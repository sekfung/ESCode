import { useCallback, useState } from "react";
import { CircuitBoardIcon, MonitorIcon } from "lucide-react";
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
 * Desktop（有 CircuitService）：点击经服务把 irId 解析成源文件路径，再打开右侧电路面板；
 * Web / 手机（没有 CircuitService）：只显示静态降级卡片（标题、器件数、网络数、"仅 Desktop 可用"），
 * 不加载面板、不订阅事件、也不向服务发起任何请求。
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
  const counts = [
    reference.componentCount === undefined
      ? null
      : intl.formatMessage({ id: "circuit.card.componentCount" }, { count: reference.componentCount }),
    reference.netCount === undefined
      ? null
      : intl.formatMessage({ id: "circuit.card.netCount" }, { count: reference.netCount }),
  ].filter((item): item is string => item !== null);

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

  if (!desktopCircuitAvailable || !circuitService)
    return (
      <section
        className="my-3 flex w-full items-center gap-3 rounded-xl border border-card-border bg-card px-3 py-2"
        data-testid="circuit-card"
        data-circuit-card-mode="static"
      >
        <CircuitBoardIcon className="size-5 shrink-0 text-foreground-subtle" />
        <div className="min-w-0 flex-1">
          <p className="truncate text-ui-base">{title}</p>
          <p className="text-ui-sm text-foreground-subtle">
            {counts.length > 0 ? `${counts.join(" · ")} · ` : ""}
            <span className="inline-flex items-center gap-1">
              <MonitorIcon className="size-3" />
              {intl.formatMessage({ id: "circuit.card.desktopOnly" })}
            </span>
          </p>
        </div>
      </section>
    );

  return (
    <section
      className="my-3 flex w-full items-center gap-3 rounded-xl border border-card-border bg-card px-3 py-2"
      data-testid="circuit-card"
      data-circuit-card-mode="desktop"
    >
      <CircuitBoardIcon className="size-5 shrink-0 text-foreground-subtle" />
      <div className="min-w-0 flex-1">
        <p className="truncate text-ui-base">{title}</p>
        <p className="text-ui-sm text-foreground-subtle">
          {counts.length > 0 ? counts.join(" · ") : reference.irId}
        </p>
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
