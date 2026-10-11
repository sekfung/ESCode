import {
  ESCODE_AGENT_PROVIDER,
  ESCODE_AGENT_PROVIDER_LABEL,
  type ESCodeProvider,
} from "@escode/shared";

export const ESCODE_AGENT_PROVIDERS: ESCodeProvider[] = [ESCODE_AGENT_PROVIDER];

export const ESCODE_AGENT_PROVIDER_LABELS: Record<ESCodeProvider, string> = {
  glm: ESCODE_AGENT_PROVIDER_LABEL,
};

export function supportsESCodeSessionFork(provider: ESCodeProvider): boolean {
  void provider;
  // 功能开关：fork 入口恢复对所有 ESCode provider 开放；实际可操作性仍由 task 状态、
  // assistant 目标和运行中保护 guard 控制，避免把 active/failed 分支暴露为可 fork。
  return true;
}

export function shouldHideESCodeSessionForkAction(provider: ESCodeProvider): boolean {
  return !supportsESCodeSessionFork(provider);
}
