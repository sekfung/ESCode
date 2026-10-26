import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { DEFAULT_WORKSPACE, waitForWorkspaceApp } from "../helpers/desktop-app.js";
import { getPassiveAgentStartupWorkspacePaths } from "../helpers/passive-agent-startup-fixture.js";
import { sendV4Prompt, waitForV4AssistantMessageContaining } from "../helpers/v4-conversation.js";
import {
  openWorkspaceDraftFromSidebar,
  waitForActiveWorkspacePath,
  waitForRestoredWorkspaceItems,
} from "../helpers/workspace-agent-warmup.js";
import { isProcessAlive, sampleProcessTree } from "./app-perf-process-tree.js";

// App 级「每多一个 workspace」与「8 workspace 优雅退出」测量
// （docs/specs/rust-unified-host.md §5）。
//
// 与 app-perf-streaming.perf.ts 一样是测量而非功能断言：`.perf.ts` 不进默认 E2E 发现，
// 只在显式 --spec 或 CI perf 作业运行。前提是 wdio.conf 的 APP_PERF_WORKSPACES_SPEC 分支已
// 播种 8 个已恢复 workspace（lastActiveTabIndex=0）。
//
// 口径：
// - 每个 workspace 打开后真实跑一轮会话（fixture `E2E_APP_PERF_WS_<n>`），让该 workspace 的
//   runtime 实例常驻；只切草稿不建实例，测不到「每多一个 workspace」的成本（首轮实测内存持平）。
// - 检查点 N=1/4/8：稳定 5s 后取两次进程树采样的内存中位数，同时记录打开的 tab 数。
// - 「每多一个 workspace」= (N=8 稳定态工作集 − N=1 稳定态工作集) / 7。
// - 「退出」= 主进程 `app.quit()` 调用到主进程 pid 消失（僵尸视为已退出）的间隔。

const WORKSPACE_COUNT = 8;
const CHECKPOINTS = [1, 4, 8] as const;
const SETTLE_MS = 5_000;
const SAMPLES = 5;
const EXIT_TIMEOUT_MS = 30_000;

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

function artifactDir(): string {
  const configured = process.env.ESCODE_E2E_ARTIFACT_DIR?.trim();
  const dir = configured ? resolve(configured) : resolve(process.cwd(), ".e2e-artifacts");
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  return dir;
}

function median(values: number[]): number {
  const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
  if (sorted.length === 0) return 0;
  const middle = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 0) {
    return ((sorted[middle - 1] ?? 0) + (sorted[middle] ?? 0)) / 2;
  }
  return sorted[middle] ?? 0;
}

async function readOpenTabCount(): Promise<number> {
  return browser.execute(() => {
    const store = (
      window as Window & {
        __escodeTabStoreE2E?: { getState?: () => { tabs?: unknown[] } };
      }
    ).__escodeTabStoreE2E;
    return store?.getState?.().tabs?.length ?? 0;
  });
}

/** 最小二乘斜率：比首末两点相减更能抵抗单点噪声（4→8 曾出现内存回落）。 */
function slope(points: Array<{ x: number; y: number }>): number {
  if (points.length < 2) return 0;
  const n = points.length;
  const sumX = points.reduce((sum, point) => sum + point.x, 0);
  const sumY = points.reduce((sum, point) => sum + point.y, 0);
  const sumXY = points.reduce((sum, point) => sum + point.x * point.y, 0);
  const sumXX = points.reduce((sum, point) => sum + point.x * point.x, 0);
  const denominator = n * sumXX - sumX * sumX;
  return denominator === 0 ? 0 : (n * sumXY - sumX * sumY) / denominator;
}

/** Host 与 Agent 进程（§5「每多一个 workspace」的成本来源）；renderer 不计入。 */
function isHostOrAgentProcess(process: { name: string; commandLine: string }): boolean {
  return /escode-host|escode-cli-rust|zcode\.cjs|escode-cli|app-server/u.test(
    `${process.name} ${process.commandLine}`,
  );
}

describe("APP-PERF: workspace 增量与优雅退出", () => {
  it("测量 N=1/4/8 的 App 进程树内存与 8 workspace 优雅退出耗时", async function () {
    this.timeout(12 * 60_000);
    const workspacePaths = getPassiveAgentStartupWorkspacePaths(
      dirname(DEFAULT_WORKSPACE),
      WORKSPACE_COUNT,
    );
    const firstWorkspace = workspacePaths[0];
    if (!firstWorkspace) throw new Error("workspace fixture 缺少第 1 个 workspace");

    await waitForWorkspaceApp(firstWorkspace, 60_000);
    await waitForRestoredWorkspaceItems(workspacePaths, 60_000);
    await waitForActiveWorkspacePath(firstWorkspace);

    const mainPid = await browser.electron.execute(
      (electron) =>
        electron.app.getAppMetrics().find((metric) => metric.type === "Browser")?.pid ?? 0,
    );
    expect(mainPid).toBeGreaterThan(0);

    const checkpoints: Array<{
      workspaceCount: number;
      openTabs: number;
      workingSetMiB: number;
      hostAndAgentMiB: number;
      cpuPercent: number;
      processCount: number;
    }> = [];
    const measure = async (workspaceCount: number) => {
      await sleep(SETTLE_MS);
      const samples = [];
      for (let index = 0; index < SAMPLES; index++) {
        samples.push(sampleProcessTree(mainPid));
        if (index < SAMPLES - 1) await sleep(1_000);
      }
      const last = samples.at(-1);
      const previous = samples.at(-2);
      const cpuPercent =
        last && previous
          ? sampleProcessTree(mainPid, { at: previous.at, cpuSeconds: previous.cpuSeconds })
              .cpuPercent
          : 0;
      checkpoints.push({
        workspaceCount,
        openTabs: await readOpenTabCount(),
        workingSetMiB: median(samples.map((sample) => sample.workingSetMiB)),
        hostAndAgentMiB: median(
          samples.map(
            (sample) =>
              sample.processes
                .filter(isHostOrAgentProcess)
                .reduce((sum, process) => sum + process.bytes, 0) /
              1024 /
              1024,
          ),
        ),
        cpuPercent,
        processCount: last?.processes.length ?? 0,
      });
    };

    for (let index = 0; index < workspacePaths.length; index++) {
      const workspacePath = workspacePaths[index];
      if (!workspacePath) throw new Error(`workspace fixture 缺少 index=${index}`);
      if (index > 0) {
        await openWorkspaceDraftFromSidebar(workspacePath);
        await waitForActiveWorkspacePath(workspacePath);
      }
      const lane = index + 1;
      await sendV4Prompt(`E2E_APP_PERF_WS_${lane}`);
      await waitForV4AssistantMessageContaining(`PERF_WS_${lane}_DONE`, 120_000);
      if ((CHECKPOINTS as readonly number[]).includes(lane)) {
        await measure(lane);
      }
    }

    // 优雅退出：app.quit() 走真实 before-quit 清理；随后在 Node 侧轮询主进程消失。
    // WDIO session 会随 App 退出而结束，因此调用与轮询都吞掉 transport 错误，
    // 只用进程存活判定，避免把「会话已关闭」误判为用例失败。
    const exitStartedAt = Date.now();
    try {
      await browser.electron.execute((electron) => {
        electron.app.quit();
        return true;
      });
    } catch {
      // App 可能在 execute 返回前就开始退出，忽略 transport 关闭。
    }
    let exitMs = 0;
    while (Date.now() - exitStartedAt < EXIT_TIMEOUT_MS) {
      if (!isProcessAlive(mainPid)) {
        exitMs = Date.now() - exitStartedAt;
        break;
      }
      await sleep(50);
    }

    const first = checkpoints.find((checkpoint) => checkpoint.workspaceCount === 1);
    const last = checkpoints.find((checkpoint) => checkpoint.workspaceCount === WORKSPACE_COUNT);
    if (!first || !last) throw new Error("缺少 N=1 或 N=8 检查点");
    const runtimeLabel = process.env.ESCODE_APP_PERF_LABEL?.trim() || "default";
    // 斜率按「多出来的 workspace 数」计：N=1 是 0 个增量，N=4 是 3，N=8 是 7。
    const points = checkpoints.map((checkpoint) => ({
      x: checkpoint.workspaceCount - 1,
      app: checkpoint.workingSetMiB,
      host: checkpoint.hostAndAgentMiB,
    }));
    const result = {
      kind: "app-perf-workspaces",
      runtimeLabel,
      platform: `${process.platform}-${process.arch}`,
      recordedAt: new Date().toISOString(),
      workspaceCount: WORKSPACE_COUNT,
      settleMs: SETTLE_MS,
      sampleCount: SAMPLES,
      checkpoints,
      // 主指标：最小二乘斜率（App 全进程树 / Host+Agent 子集）。
      perWorkspaceMiB: slope(points.map((point) => ({ x: point.x, y: point.app }))),
      perWorkspaceHostMiB: slope(points.map((point) => ({ x: point.x, y: point.host }))),
      // 端点差仅作交叉检查：8 个 workspace 的内存不一定单调，端点差会受噪声放大。
      perWorkspaceEndpointMiB: (last.workingSetMiB - first.workingSetMiB) / (WORKSPACE_COUNT - 1),
      exitMs,
      exitTimedOut: exitMs === 0,
    };
    const file = join(
      artifactDir(),
      `app-perf-workspaces.${runtimeLabel}.${process.platform}-${process.arch}.json`,
    );
    writeFileSync(file, JSON.stringify(result, null, 2) + "\n");
    console.log(`[app-perf] ${file}`);
    console.log(
      `[app-perf] workspaces ${checkpoints
        .map(
          (checkpoint) =>
            `${checkpoint.workspaceCount}:${checkpoint.workingSetMiB.toFixed(1)}MiB/tabs=${checkpoint.openTabs}/procs=${checkpoint.processCount}`,
        )
        .join(" ")}; perWorkspace=${result.perWorkspaceMiB.toFixed(2)}MiB ` +
        `(host+agent=${result.perWorkspaceHostMiB.toFixed(2)}MiB, endpoint=${result.perWorkspaceEndpointMiB.toFixed(2)}MiB); ` +
        `exit=${exitMs}ms`,
    );

    expect(checkpoints.map((checkpoint) => checkpoint.workspaceCount)).toEqual([...CHECKPOINTS]);
    expect(last.openTabs).toBeGreaterThanOrEqual(WORKSPACE_COUNT);
    expect(exitMs).toBeGreaterThan(0);
  });
});
