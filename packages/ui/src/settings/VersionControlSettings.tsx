import { useEffect, useState } from "react";
import {
  WORKSPACE_VCS_OVERRIDES,
  type WorkspaceVcsInfo,
  type WorkspaceVcsOverride,
} from "@escode/shared";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select.js";
import { Switch } from "@/components/ui/switch.js";
import { useWorkspaceServicesResolution } from "@/hooks/useWorkspaceServices.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";
import { notifyWorkspaceVcsChanged } from "@/svn/workspaceVcsEvents.js";
import { SettingsGroupCard, SettingsRow } from "@/settings/SettingsPageParts.js";

/**
 * 设置页“版本控制”分组（spec：docs/svn/ui.md §9）。覆盖值由当前 workspace 所在 Host 的
 * workspace-vcs 服务持久化；写入后广播通知，让 App 中的判定所有者重新读取，不在这里保存第二份。
 */
export function VersionControlSettings({
  workspacePath,
  workspaceIdentity,
  remoteSessionId,
  remoteCheckEnabled,
  onRemoteCheckEnabledChange,
}: {
  workspacePath?: string | null;
  workspaceIdentity?: string;
  remoteSessionId?: string;
  remoteCheckEnabled: boolean;
  onRemoteCheckEnabledChange: (enabled: boolean) => void;
}) {
  const { intl } = useESCodeIntl();
  // 覆盖值属于 workspace 所在 Host：远程 workspace 必须解析到远端服务，不能写本机设置。
  const { services } = useWorkspaceServicesResolution(
    workspacePath,
    remoteSessionId,
    workspaceIdentity,
  );
  const workspaceVcsService = workspacePath ? services.workspaceVcsService : undefined;
  const [info, setInfo] = useState<WorkspaceVcsInfo | null>(null);

  useEffect(() => {
    setInfo(null);
    if (!workspacePath || !workspaceVcsService) {
      return;
    }
    let cancelled = false;
    void workspaceVcsService
      .getWorkspaceVcsInfo({ workspacePath, ...(workspaceIdentity ? { workspaceIdentity } : {}) })
      .then((next) => {
        if (!cancelled) {
          setInfo(next);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [workspaceIdentity, workspacePath, workspaceVcsService]);

  const handleOverrideChange = async (override: WorkspaceVcsOverride) => {
    if (!workspacePath || !workspaceVcsService) {
      return;
    }
    try {
      const next = await workspaceVcsService.setWorkspaceVcsOverride({
        workspacePath,
        ...(workspaceIdentity ? { workspaceIdentity } : {}),
        override,
      });
      setInfo(next);
      notifyWorkspaceVcsChanged(workspaceIdentity?.trim() || workspacePath);
    } catch (error) {
      logger.warn("[VersionControlSettings] 保存版本控制类型失败", {
        error: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <SettingsGroupCard>
      {workspacePath && workspaceVcsService ? (
        <SettingsRow
          label={intl.formatMessage({ id: "settings.vcs.type" })}
          description={intl.formatMessage(
            { id: "settings.vcs.typeDescription" },
            {
              detected: info
                ? intl.formatMessage({ id: `settings.vcs.kind.${info.kind}` })
                : intl.formatMessage({ id: "common.loading" }),
            },
          )}
          control={
            <Select
              value={info?.override ?? "auto"}
              disabled={!info}
              onValueChange={(value) => void handleOverrideChange(value as WorkspaceVcsOverride)}
            >
              <SelectTrigger className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                {WORKSPACE_VCS_OVERRIDES.map((value) => (
                  <SelectItem key={value} value={value}>
                    {intl.formatMessage({ id: `settings.vcs.override.${value}` })}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          }
        />
      ) : null}
      <SettingsRow
        label={intl.formatMessage({ id: "settings.vcs.svnRemoteCheck" })}
        description={intl.formatMessage({ id: "settings.vcs.svnRemoteCheckDescription" })}
        control={
          <Switch
            aria-label={intl.formatMessage({ id: "settings.vcs.svnRemoteCheck" })}
            checked={remoteCheckEnabled}
            onCheckedChange={onRemoteCheckEnabledChange}
          />
        }
      />
    </SettingsGroupCard>
  );
}
