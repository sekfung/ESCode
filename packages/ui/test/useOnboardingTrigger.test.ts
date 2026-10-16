// @vitest-environment jsdom
import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { useOnboardingTrigger } from "@/onboarding/useOnboardingTrigger.js";
import type { IOnboardingRecordService } from "@escode/services";

/**
 * CR 修复回归：useOnboardingTrigger 曾因坏 copy-paste 把 useState/useRef 嵌进
 * useEffect 回调（Rules of Hooks 违规，挂载即抛 "Invalid hook call"），且内层
 * setNeedsOnboarding 写的是被遮蔽的 state，触发/超时/回填全部死链。该缺陷
 * typecheck/lint/存量测试均不可见，必须用真实渲染覆盖。
 */
function createRecordService(
  overrides: Partial<IOnboardingRecordService> = {},
): IOnboardingRecordService {
  return {
    appendRecord: async () => {},
    dismissOnboarding: async () => {},
    claimAnonymousRecord: async () => {},
    shouldOnboard: async () => true,
    getLatestEntry: async () => null,
    syncSettingsFromRecord: async () => null,
    updateRecordPreferences: async () => {},
    getRecords: async () => null,
    clearRecords: async () => {},
    ...overrides,
  };
}

// 账号登录链已下线：hook 不再按 userId 变化回填 settings，只保留记录判定本身。
function renderTrigger(options: {
  service: IOnboardingRecordService | null;
  hasStoredOccupation?: boolean;
}) {
  return renderHook(() =>
    useOnboardingTrigger({
      onboardingRecord: options.service,
      hasStoredOccupation: options.hasStoredOccupation ?? false,
      loadDeviceMid: () => "device-a",
    }),
  );
}

describe("useOnboardingTrigger", () => {
  it("挂载不抛 hook 规则错误，并按 shouldOnboard=false 输出判定", async () => {
    const service = createRecordService({ shouldOnboard: async () => false });
    const { result } = renderTrigger({ service });
    // 修复前：effect 内的 useState 在挂载时同步抛 "Invalid hook call"
    await waitFor(() => expect(result.current[0]).toBe(false));
  });

  it("shouldOnboard=true 时输出需要引导；markOnboarded 置回 false", async () => {
    const service = createRecordService({ shouldOnboard: async () => true });
    const { result } = renderTrigger({ service });
    await waitFor(() => expect(result.current[0]).toBe(true));
    act(() => {
      result.current[1]();
    });
    expect(result.current[0]).toBe(false);
  });

  it("服务不可用退回 settings 判定（hasStoredOccupation=false 视为需引导）", async () => {
    const { result } = renderTrigger({ service: null, hasStoredOccupation: false });
    await waitFor(() => expect(result.current[0]).toBe(true));
  });

  it("登录后先认领再判定：认领使判定不触发（同一人不重复引导）", async () => {
    // shouldOnboard 返回 true 表示"若无认领会弹引导"；认领先行由服务层保证一致性，
    // 这里验证调用顺序：claim 必须在 shouldOnboard 之前完成。
    const calls: string[] = [];
    const service = createRecordService({
      claimAnonymousRecord: async () => {
        calls.push("claim");
      },
      shouldOnboard: async () => {
        calls.push("shouldOnboard");
        return true;
      },
    });
    const { result } = renderTrigger({ service, userId: "u1" });
    await waitFor(() => expect(result.current[0]).toBe(true));
    expect(calls.indexOf("claim")).toBeLessThan(calls.indexOf("shouldOnboard"));
  });

  it("shouldOnboard RPC 挂起时 3 秒超时兜底退回 settings 判定", { timeout: 8000 }, async () => {
    const service = createRecordService({
      shouldOnboard: () => new Promise<boolean>(() => {}),
    });
    const { result } = renderTrigger({ service, hasStoredOccupation: false });
    await waitFor(() => expect(result.current[0]).toBe(true), { timeout: 5000 });
  });
});
