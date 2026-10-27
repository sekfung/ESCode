import type { LaunchMarks } from "@escode/shared";

/**
 * bench 冷启动阶段日志（docs/specs/rust-u4-startup-phases.md）：T0 进程创建 → T1 main JS →
 * T2 app ready → T3 loadURL → T4 renderer bundle 开始执行 → T5 React 首次 commit。
 * 只在主进程注入 `benchReady=1` 时输出，生产不产生日志。
 */
export function reportBenchStartupPhases(
  launchMarks: LaunchMarks | null,
  rendererStartedAt: number,
  reactCommitAt: number,
): void {
  if (!launchMarks) return;
  if (new URLSearchParams(window.location.search).get("benchReady") !== "1") return;
  const phases = {
    processToMain: launchMarks.mainStart - launchMarks.createdAt,
    mainToAppReady: launchMarks.appReady - launchMarks.mainStart,
    appReadyToLoadUrl: launchMarks.loadUrl - launchMarks.appReady,
    loadUrlToRenderer: rendererStartedAt - launchMarks.loadUrl,
    rendererToReactCommit: reactCommitAt - rendererStartedAt,
    totalToReactCommit: reactCommitAt - launchMarks.createdAt,
  };
  console.info(
    `[bench-startup] ${Object.entries(phases)
      .map(([key, value]) => `${key}=${value}ms`)
      .join(" ")}`,
  );
}
