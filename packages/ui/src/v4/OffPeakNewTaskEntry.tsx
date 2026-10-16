/* New task 页闲时任务入口（spec §5「模板」/ D30-4 第二入口）。
   引导横幅 + Client Scenes 模板卡（预填创建表单，Customize=空白）。
   灰度未命中不渲染（D31）；横幅可关闭（session 级，D28）。
   点击卡片：暂存预填草稿 → 打开 Automations 主视图（idle tab）创建表单消费草稿。
   套餐族删除（2026-10-15）：Coding Plan 判定/升级引导/漏斗埋点随套餐移除，
   入口灰度（client-configs 的 offPeak 曝光键）保留。 */
import { Megaphone, Moon, X } from "lucide-react";
import { ClientSceneLucideIcon } from "@/components/ClientSceneLucideIcon.js";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { useProviderSettingsView } from "@/hooks/useProviderSettingsView.js";
import { useOffPeakEligibility } from "@/hooks/useOffPeakEligibility.js";
import { useServices } from "@/hooks/useServices.js";
import { useSettings } from "@/hooks/useSettingService.js";
import {
  appendLocalCustomizeTemplate,
  materializeOffPeakTemplateDraft,
  resolveOffPeakTemplateText,
  type OffPeakAutomationTemplate,
} from "@/settings/automationTemplateCatalog.js";
import { useAutomationTemplates } from "@/settings/useAutomationTemplates.js";
import { useOffPeakTaskStore, type OffPeakCreateDraft } from "@/store/offPeakTaskStore.js";
import { useMemo } from "react";

interface OffPeakNewTaskEntryProps {
  /** 打开 Automations 主视图（idle tab）；点击卡片前会先暂存预填草稿。 */
  onOpenAutomations?: () => void;
}

export function OffPeakNewTaskEntry({ onOpenAutomations }: OffPeakNewTaskEntryProps) {
  const { intl, locale } = useESCodeIntl();
  const { clientScenesService } = useServices();
  const { offPeak: remoteOffPeakTemplates } = useAutomationTemplates(clientScenesService);
  const offPeakTemplates = useMemo(
    () => appendLocalCustomizeTemplate(remoteOffPeakTemplates),
    [remoteOffPeakTemplates],
  );
  const { settings: sharedSettings } = useSettings();
  const providerSettingsRead = useProviderSettingsView();
  const providerSettingsView =
    providerSettingsRead.state.status === "ready" ? providerSettingsRead.state.view : null;
  useOffPeakEligibility(sharedSettings, providerSettingsView?.revision);
  const grayConfig = useOffPeakTaskStore((state) => state.grayConfig);
  const dismissed = useOffPeakTaskStore((state) => state.newTaskBannerDismissed);
  const dismiss = useOffPeakTaskStore((state) => state.dismissNewTaskBanner);
  const setPendingCreateDraft = useOffPeakTaskStore((state) => state.setPendingCreateDraft);

  // 灰度未命中 / 用户已关闭 → 不渲染（D31 曝光门控 + D28 可关闭）。
  if (grayConfig?.enabled !== true || dismissed) return null;

  const handleCardClick = (template: OffPeakAutomationTemplate) => {
    // Bugfix：首页文案走 i18n、草稿却曾写死英文，导致中文系统点击模板后创建页中英混排。
    // 点击时按当前 locale 生成同源标题与指令；Customize 仍保持空白创建。
    const materialized = materializeOffPeakTemplateDraft(template, locale);
    const draft: OffPeakCreateDraft = {
      ...(template.customize ? {} : { title: materialized.title, prompt: materialized.prompt }),
      telemetrySource: {
        eventRegion: "app.session",
        templateId: template.id,
      },
    };
    setPendingCreateDraft(draft);
    onOpenAutomations?.();
  };

  return (
    // 5488-3978：入口常驻草稿页底部，pb-[38px] 给出与窗口底部的固定间距；
    // 宽度不依赖 @container/conversation 断点外扩（该容器只声明在 timeline 内部）。
    <div
      data-off-peak-new-task-entry="true"
      className="flex w-full shrink-0 flex-col items-center gap-3 px-4 pb-[38px]"
    >
      {/* 引导横幅：32px 图标位 + 主文案/说明 Tooltip，整体水平居中 */}
      <div
        data-off-peak-new-task-banner="true"
        className="flex w-full max-w-[580px] items-center justify-center"
      >
        <div className="flex min-w-0 items-center gap-1 text-foreground-subtle opacity-80">
          <span className="flex size-8 shrink-0 items-center justify-center" aria-hidden="true">
            <Megaphone className="size-4" strokeWidth={2} />
          </span>
          <TooltipProvider>
            <Tooltip>
              <TooltipTrigger asChild>
                <span
                  tabIndex={0}
                  className="min-w-0 cursor-default rounded-sm text-ui-caption font-normal leading-snug focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-border-hover"
                >
                  {intl.formatMessage({ id: "offPeak.newTask.bannerText" })}
                </span>
              </TooltipTrigger>
              <TooltipContent side="top" sideOffset={6} className="max-w-none whitespace-nowrap">
                <span>
                  {intl.formatMessage({
                    id: "offPeak.newTask.bannerTipText",
                  })}
                </span>
              </TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </div>
        <button
          type="button"
          data-off-peak-new-task-dismiss="true"
          onClick={dismiss}
          aria-label={intl.formatMessage({ id: "common.close" })}
          className="ml-auto hidden size-8 shrink-0 items-center justify-center rounded-md text-foreground-subtle opacity-80 hover:bg-hover hover:opacity-100"
        >
          <X className="size-4" strokeWidth={2} aria-hidden="true" />
        </button>
      </div>

      {/* 远程模板按钮使用等宽栅格；桌面横排，窄屏单列。 */}
      <div
        data-off-peak-template-grid="true"
        className="grid w-full max-w-[780px] grid-cols-1 gap-4 sm:grid-cols-3"
      >
        {offPeakTemplates.map((template) => (
          <button
            key={template.id}
            type="button"
            onClick={() => handleCardClick(template)}
            className="flex h-full w-full flex-col gap-2 rounded-2xl border border-card-border bg-background p-3 text-left transition-colors hover:bg-surface-hover"
          >
            <div className="flex items-center gap-1.5 text-foreground">
              <span className="flex size-5 shrink-0 items-center justify-center" aria-hidden="true">
                <ClientSceneLucideIcon
                  name={template.iconName}
                  aria-hidden="true"
                  className="size-4"
                  size={16}
                  strokeWidth={2}
                  fallback={
                    <Moon
                      aria-hidden="true"
                      className="size-4"
                      data-off-peak-homepage-template-icon="moon"
                      size={16}
                      strokeWidth={2}
                    />
                  }
                />
              </span>
              <span className="truncate text-ui-base leading-5 text-foreground">
                {resolveOffPeakTemplateText(template, "title", locale, intl.formatMessage)}
              </span>
            </div>
            <p className="text-wrap-phrase line-clamp-2 text-ui-caption font-normal leading-snug text-foreground-subtle">
              {resolveOffPeakTemplateText(
                template,
                "homepageDescription",
                locale,
                intl.formatMessage,
              )}
            </p>
          </button>
        ))}
      </div>
    </div>
  );
}
