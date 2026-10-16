import { useEffect, useState } from "react";
import type { useOnboardingRecordService } from "@/hooks/useOnboardingRecordService.js";
import { logger } from "@/logger.js";

/**
 * 引导触发判定：当前用户在本地记录里没有条目时
 * needsOnboarding=true。手动修改由各入口回写 record（updateRecordPreferences），
 * record 始终等于该用户最新偏好。
 *
 * 返回 [needsOnboarding, markOnboarded]：null 表示异步判定中；markOnboarded 在引导
 * 保存成功后把判定置 false（记录已落盘，本次会话不再触发）。
 */
export function useOnboardingTrigger(options: {
  onboardingRecord: ReturnType<typeof useOnboardingRecordService>;
  hasStoredOccupation: boolean;
  loadDeviceMid: () => string;
}): [boolean | null, () => void] {
  const { onboardingRecord, hasStoredOccupation, loadDeviceMid } = options;
  // null 表示异步判定中（触发判定改为按本地记录，docs/onboarding-record-spec.md）。
  const [needsOnboarding, setNeedsOnboarding] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    const fallback = () => !hasStoredOccupation;
    // 服务不可用（旧测试 double / 未注册的 host）时退回旧 settings 判定，行为不回退。
    if (!onboardingRecord) {
      setNeedsOnboarding(fallback());
      return;
    }
    // shouldOnboard 走 RPC，host 未带上 onboarding-record channel 时调用会挂起，
    // 之前判定期间渲染 null 会把整个主界面拦成永久黑屏。加超时兜底退回 settings 判定，
    // 保证任何情况下主界面最多等 3 秒。
    const timeout = setTimeout(() => {
      if (!cancelled) {
        logger.warn("[occupation-onboarding] shouldOnboard 超时，退回 settings 判定");
        setNeedsOnboarding(fallback());
      }
    }, 3000);
    // 登录认领先行：未登录时答的引导（null 条目）移交给当前登录用户，同一人不重复引导。
    // 必须 await 完成后再判定，否则 shouldOnboard 读到认领前的文件会误判需要引导。
    onboardingRecord
      .claimAnonymousRecord()
      .catch((cause: unknown) => {
        logger.warn("[occupation-onboarding] 认领匿名引导记录失败", { error: String(cause) });
      })
      .then(() => onboardingRecord.shouldOnboard(loadDeviceMid()))
      .then(
        (result) => {
          if (!cancelled) setNeedsOnboarding(result);
        },
        (cause) => {
          logger.warn("[occupation-onboarding] shouldOnboard 检查失败", { error: String(cause) });
          if (!cancelled) setNeedsOnboarding(fallback());
        },
      )
      .finally(() => clearTimeout(timeout));
    return () => {
      cancelled = true;
      clearTimeout(timeout);
    };
    // 不依赖 hasStoredOccupation（对应 settings?.onboardingOccupation）：保存成功会改写该字段，
    // 若记录写入失败会在当场重开引导；记录缺失导致的再次触发按约定留给下次启动。
  }, [onboardingRecord, loadDeviceMid]);
  return [needsOnboarding, () => setNeedsOnboarding(false)];
}
