interface RootStartupGateState {
  isResolvingStartupAuthState: boolean;
  isResolvingProviderStartupState: boolean;
  isRestoring: boolean;
  isBootstrappingInitialWorkspace: boolean;
}

interface RootStartupLoadingVisibilityState extends RootStartupGateState {
  isDesktop: boolean | undefined;
  welcomeScreenOpen: boolean;
}

interface FallbackWorkspaceCreateState {
  isMounted: boolean;
  activeWorkspacePath: string | null;
}

interface ProviderStartupSyncState {
  providerFamilyDomainMigrationComplete: boolean;
  modelSelectionViewHydrated: boolean;
}

interface ProviderAvailabilityLoginEntryGuardState {
  isWebRemoteControlRoot: boolean;
}

interface ProviderStartupResolutionState {
  isWebRemoteControlRoot: boolean;
  providerStartupSyncPending: boolean;
  providerAvailabilityStartupCheckCompleted: boolean;
}

/** 由账号/供应商状态自动推导出的登录入口理由；用户显式发起的入口不在其中。 */
export type AutomaticLoginEntryReason =
  | "startup-provider-required"
  | "session-expired"
  | "logout-provider-required";

/**
 * 自动登录入口只在官方账号登录策略启用时成立。
 * 免登录发行版保留这些理由的全部判定链路，但不把用户带离主界面；
 * 显式入口（`manual-login` / `provider-request`）不经过这里，始终可用。
 */
export function resolveAutomaticLoginEntryReason(
  reason: AutomaticLoginEntryReason,
  officialAccountLoginRequired: boolean,
): AutomaticLoginEntryReason | null {
  return officialAccountLoginRequired ? reason : null;
}

export function shouldBlockRootRender(state: RootStartupGateState): boolean {
  return (
    state.isResolvingStartupAuthState ||
    state.isResolvingProviderStartupState ||
    state.isRestoring ||
    state.isBootstrappingInitialWorkspace
  );
}

export function shouldShowRootStartupLoading(state: RootStartupLoadingVisibilityState): boolean {
  // 修复原因：登录入口是启动门禁的结果，不是可继续被门禁遮挡的后台状态。
  // 如果 WelcomeScreen 已经打开，继续返回启动 loading 会把未登录用户卡在黑屏 logo。
  return Boolean(state.isDesktop) && !state.welcomeScreenOpen && shouldBlockRootRender(state);
}

export function shouldEnableProviderAvailabilityLoginEntryGuard(
  state: ProviderAvailabilityLoginEntryGuardState,
): boolean {
  // 修复原因：手机 Web 远控只 attachment 到桌面已有 host，不能用本地空 provider
  // 快照触发登录页，否则断连/home-only 首页会被 WelcomeScreen 覆盖。
  return !state.isWebRemoteControlRoot;
}

export function shouldResolveProviderStartupState(state: ProviderStartupResolutionState): boolean {
  if (state.isWebRemoteControlRoot) {
    // 修复原因：Web 远控的 workspace tab 由 shared-host bridge 注入；
    // provider 可用性属于桌面 host 侧状态，不应阻断手机端 paired loading / workspace 注入。
    return false;
  }

  return state.providerStartupSyncPending || !state.providerAvailabilityStartupCheckCompleted;
}

export function shouldOpenFallbackWorkspaceAfterCreate(
  state: FallbackWorkspaceCreateState,
): boolean {
  return state.isMounted && !state.activeWorkspacePath;
}

export function isProviderStartupSyncPending(state: ProviderStartupSyncState): boolean {
  return !state.providerFamilyDomainMigrationComplete || !state.modelSelectionViewHydrated;
}
