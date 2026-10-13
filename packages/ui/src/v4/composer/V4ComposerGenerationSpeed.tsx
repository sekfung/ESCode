import { cn } from "@/components/lib/utils.js";
import { useESCodeIntl } from "@/i18n/IntlProvider.js";
import { useLocalGenerationSpeed } from "@/v4/telemetry/useLocalGenerationSpeed.js";

/** 占位符：速度未定义时的显示（规格 2.1）——窗口排空、等待首 token、或缺权威读数。 */
const SPEED_PLACEHOLDER = "—";

/** 显示格式：整数；小于 1 时保留一位小数（规格 2.1）。 */
function formatSpeedValue(value: number): string {
  return value < 1 ? value.toFixed(1) : String(Math.round(value));
}

/**
 * 输入框工具条里的会话生成速度读数（规格 2.2：位于上下文容量左侧、独立于 modelLocked 渲染）。
 *
 * 实时值来自 Renderer 本地观测、轮末解码速度读自 `turnHeader` 的 CLI 投影字段——两个量、两个所有者
 * （`localGenerationSpeed.ts`，规格 4）。读数不写会话状态、不进 Composer 的 memo 依赖。
 */
export function V4ComposerGenerationSpeed({ sessionId }: { sessionId: string }) {
  const { intl } = useESCodeIntl();
  const view = useLocalGenerationSpeed(sessionId);
  const unit = intl.formatMessage({ id: "chat.generationSpeed.unit" });
  const displayValue = view.value === null ? SPEED_PLACEHOLDER : formatSpeedValue(view.value);
  const visible = view.value === null ? SPEED_PLACEHOLDER : `${displayValue} ${unit}`;
  const label =
    view.mode === "none"
      ? intl.formatMessage({ id: "chat.generationSpeed.unknown" })
      : intl.formatMessage({ id: `chat.generationSpeed.${view.mode}` }, { value: visible });

  return (
    <span
      aria-label={label}
      className={cn(
        "shrink-0 font-mono text-ui-xs tabular-nums",
        // 规格 2.3：实时与保留态用同一个格式，靠颜色强弱区分（DESIGN.md 的次要文案/弱元数据）。
        view.mode === "average" ? "text-foreground-subtlest" : "text-foreground-subtle",
      )}
      data-generation-speed-mode={view.mode}
      data-generation-speed-value={view.value === null ? "" : displayValue}
      title={label}
    >
      {visible}
    </span>
  );
}
