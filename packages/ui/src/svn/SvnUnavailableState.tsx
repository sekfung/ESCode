import { useState, type FormEvent } from "react";
import type { ISvnService } from "@escode/services";
import type { SvnWorkingCopyInfo } from "@escode/shared";
import { AlertCircleIcon } from "lucide-react";
import { Button } from "@/components/ui/button.js";
import { Input } from "@/components/ui/input.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { logger } from "@/logger.js";

/**
 * SVN 不可用时的说明：未安装 / 版本过低 / Host 不支持 / 不是工作副本（spec：docs/svn/ui.md §3.7）。
 * svn 路径保存在 workspace 所在 Host 的设置里，因此入口放在这里而不是全局设置页。
 */
export function SvnUnavailableState({
  info,
  loading,
  svnService,
  onRefresh,
}: {
  info: SvnWorkingCopyInfo | null;
  loading: boolean;
  svnService: ISvnService | null;
  onRefresh: () => void;
}) {
  const { intl } = useESCodeIntl();
  const [binaryPath, setBinaryPath] = useState("");
  const [saving, setSaving] = useState(false);

  if (!info || loading) {
    return (
      <div className="flex h-full items-center justify-center text-ui-base text-foreground-subtle">
        {intl.formatMessage({ id: "common.loading" })}
      </div>
    );
  }

  const status = info.availability.status;
  const titleId =
    status === "not-found"
      ? "svn.unavailable.notFoundTitle"
      : status === "unsupported-version"
        ? "svn.unavailable.versionTitle"
        : status === "unsupported-host"
          ? "svn.unavailable.hostTitle"
          : "svn.unavailable.notWorkingCopyTitle";
  const description =
    status === "not-found"
      ? intl.formatMessage({ id: "svn.unavailable.notFoundDescription" })
      : status === "unsupported-version"
        ? intl.formatMessage(
            { id: "svn.unavailable.versionDescription" },
            { version: info.availability.version },
          )
        : status === "unsupported-host"
          ? intl.formatMessage({ id: "svn.unavailable.hostDescription" })
          : intl.formatMessage({ id: "svn.unavailable.notWorkingCopyDescription" });
  const canConfigureBinary =
    Boolean(svnService) && (status === "not-found" || status === "unsupported-version");

  const handleSave = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!svnService) {
      return;
    }
    setSaving(true);
    try {
      await svnService.setBinaryPath({ binaryPath });
      onRefresh();
    } catch (error) {
      logger.warn("[SvnPane] 保存 svn 路径失败", {
        error: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
      <AlertCircleIcon className="size-8 text-foreground-subtlest" />
      <p className="text-ui-base font-medium text-foreground">
        {intl.formatMessage({ id: titleId })}
      </p>
      <p className="whitespace-pre-line text-ui-base text-foreground-subtle">{description}</p>
      {canConfigureBinary ? (
        <form className="flex w-full max-w-sm items-center gap-2" onSubmit={handleSave}>
          <Input
            value={binaryPath}
            placeholder={intl.formatMessage({ id: "svn.unavailable.binaryPathPlaceholder" })}
            aria-label={intl.formatMessage({ id: "svn.unavailable.binaryPathPlaceholder" })}
            onChange={(event) => setBinaryPath(event.target.value)}
          />
          <Button type="submit" variant="outline" disabled={saving}>
            {intl.formatMessage({ id: "svn.unavailable.binaryPathSave" })}
          </Button>
        </form>
      ) : null}
      <Button type="button" variant="ghost" onClick={onRefresh}>
        {intl.formatMessage({ id: "git.action.refresh" })}
      </Button>
    </div>
  );
}
