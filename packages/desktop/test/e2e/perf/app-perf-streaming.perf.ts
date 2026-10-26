import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  prepareV4ConversationE2E,
  sendV4Prompt,
  startNewV4Draft,
  waitForV4AssistantMessageContaining,
} from "../helpers/v4-conversation.js";
import { sampleProcessTree } from "./app-perf-process-tree.js";

// App 级性能测量（docs/specs/rust-unified-host.md §5「首条消息第一段」「4 路流式全进程 CPU」）。
//
// 这是测量而不是功能断言，因此：
// - 文件名用 `.perf.ts`，不进默认 E2E spec 发现（正式套件不因性能数字波动而红）；
// - 只在显式 --spec 运行时执行，由 CI 的 perf 作业或本地命令触发；
// - 迁移到「性能阈值断言」之前，先把数字落盘（ESCODE_E2E_ARTIFACT_DIR 或 .e2e-artifacts）。
//
// 前提：replay fixture 必须包含 common.json 与 perf/app-perf-streaming.json。
//   E2E_PROVIDER_REPLAY_FIXTURE_PATH=packages/desktop/test/e2e/fixtures/upstream/common.json,\
//   packages/desktop/test/e2e/fixtures/upstream/perf/app-perf-streaming.json
//
// 口径：4 路会话并发流式（每条 lane 约 12s），期间用 Electron `app.getAppMetrics()` 采样
// 全 App（main/renderer/gpu/utility/agent）CPU 与工作集；首段 = 点击发送到该 lane 首个
// assistant 文本出现在时间线的间隔。

const LANE_COUNT = 4;
const SAMPLE_INTERVAL_MS = 500;
const STREAM_WINDOW_MS = 12_000;

interface AppMetricSample {
  at: number;
  cpuPercent: number;
  workingSetMiB: number;
  processCount: number;
  processes: Array<{ pid: number; type: string; cpuPercent: number; workingSetMiB: number }>;
}

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

function artifactDir(): string {
  const configured = process.env.ESCODE_E2E_ARTIFACT_DIR?.trim();
  const dir = configured ? resolve(configured) : resolve(process.cwd(), ".e2e-artifacts");
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  return dir;
}

async function sampleAppMetrics(): Promise<AppMetricSample> {
  const metrics = await browser.electron.execute((electron) =>
    electron.app.getAppMetrics().map((metric) => ({
      pid: metric.pid,
      type: metric.type,
      cpuPercent: metric.cpu?.percentCPUUsage ?? 0,
      workingSetKiB: metric.memory?.workingSetSize ?? 0,
    })),
  );
  const processes = metrics.map((metric) => ({
    pid: metric.pid,
    type: metric.type,
    cpuPercent: metric.cpuPercent,
    workingSetMiB: metric.workingSetKiB / 1024,
  }));
  return {
    at: Date.now(),
    cpuPercent: processes.reduce((sum, value) => sum + value.cpuPercent, 0),
    workingSetMiB: processes.reduce((sum, value) => sum + value.workingSetMiB, 0),
    processCount: processes.length,
    processes,
  };
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

describe("APP-PERF: 首段延迟与 4 路流式 CPU", () => {
  it("记录首段延迟与并发流式期间的 App CPU/内存", async function () {
    this.timeout(10 * 60_000);
    await prepareV4ConversationE2E();

    const electronProcesses = await sampleAppMetrics();
    // 主进程 pid 从 Electron 自己的进程表拿；Agent/Host 是它的子进程，必须用进程树补齐。
    const mainPid = electronProcesses.processes.find((entry) => entry.type === "Browser")?.pid;
    expect(mainPid).toBeGreaterThan(0);
    const idleTree = sampleProcessTree(mainPid!);
    const idle = {
      ...electronProcesses,
      treeCpuPercent: 0,
      treeWorkingSetMiB: idleTree.workingSetMiB,
      treeProcessCount: idleTree.processes.length,
    };
    const samples: AppMetricSample[] = [];
    const treeSamples: Array<ReturnType<typeof sampleProcessTree>> = [];
    let sampling = true;
    const sampler = (async () => {
      let previousTree: { at: number; cpuSeconds: number } | undefined;
      while (sampling) {
        try {
          samples.push(await sampleAppMetrics());
        } catch {
          // 采样期间 App 可能正忙；单次失败不终止测量。
        }
        try {
          const tree = sampleProcessTree(mainPid!, previousTree);
          previousTree = { at: tree.at, cpuSeconds: tree.cpuSeconds };
          treeSamples.push(tree);
        } catch {
          // 进程可能在采样间隔内退出；跳过本次。
        }
        await sleep(SAMPLE_INTERVAL_MS);
      }
    })();

    const lanes: Array<{ lane: number; firstSegmentMs: number }> = [];
    try {
      for (let lane = 1; lane <= LANE_COUNT; lane++) {
        if (lane > 1) await startNewV4Draft();
        const startedAt = Date.now();
        await sendV4Prompt(`E2E_APP_PERF_LANE_${lane}`);
        await waitForV4AssistantMessageContaining(`PERF_LANE_${lane}_FIRST`, 120_000);
        lanes.push({ lane, firstSegmentMs: Date.now() - startedAt });
      }
      // 4 路都在流式窗口内：继续采样到窗口结束前，避免把收尾算进流式 CPU。
      const remaining = STREAM_WINDOW_MS - (Date.now() - (idle.at ?? Date.now()));
      if (remaining > 0) await sleep(remaining);
    } finally {
      sampling = false;
      await sampler;
    }

    const streamingSamples = samples.slice(1);
    const streamingTreeSamples = treeSamples.slice(2);
    const runtimeLabel = process.env.ESCODE_APP_PERF_LABEL?.trim() || "default";
    const result = {
      kind: "app-perf-streaming",
      runtimeLabel,
      platform: `${process.platform}-${process.arch}`,
      recordedAt: new Date().toISOString(),
      laneCount: LANE_COUNT,
      streamWindowMs: STREAM_WINDOW_MS,
      sampleIntervalMs: SAMPLE_INTERVAL_MS,
      idle,
      lanes,
      firstSegmentMs: {
        median: median(lanes.map((lane) => lane.firstSegmentMs)),
        max: Math.max(...lanes.map((lane) => lane.firstSegmentMs)),
        values: lanes.map((lane) => lane.firstSegmentMs),
      },
      // Electron 自有进程（main/renderer/gpu/utility Host）——app.getAppMetrics 口径。
      streaming: {
        samples: streamingSamples.length,
        cpuPercentMedian: median(streamingSamples.map((sample) => sample.cpuPercent)),
        cpuPercentMax: Math.max(0, ...streamingSamples.map((sample) => sample.cpuPercent)),
        workingSetMiBMedian: median(streamingSamples.map((sample) => sample.workingSetMiB)),
        processCountMax: Math.max(0, ...streamingSamples.map((sample) => sample.processCount)),
      },
      // 全进程口径：主进程子树（含 Host 与 Agent 进程），§5「4 路流式全进程 CPU」。
      streamingTree: {
        samples: streamingTreeSamples.length,
        cpuPercentMedian: median(streamingTreeSamples.map((sample) => sample.cpuPercent)),
        cpuPercentMax: Math.max(0, ...streamingTreeSamples.map((sample) => sample.cpuPercent)),
        workingSetMiBMedian: median(streamingTreeSamples.map((sample) => sample.workingSetMiB)),
        processCountMax: Math.max(
          0,
          ...streamingTreeSamples.map((sample) => sample.processes.length),
        ),
      },
      samples,
      treeSamples: streamingTreeSamples.map((sample) => ({
        at: sample.at,
        cpuPercent: sample.cpuPercent,
        workingSetMiB: sample.workingSetMiB,
        processes: sample.processes,
      })),
    };
    const file = join(
      artifactDir(),
      `app-perf-streaming.${runtimeLabel}.${process.platform}-${process.arch}.json`,
    );
    writeFileSync(file, JSON.stringify(result, null, 2) + "\n");
    console.log(`[app-perf] ${file}`);
    console.log(
      `[app-perf] firstSegment median=${result.firstSegmentMs.median}ms max=${result.firstSegmentMs.max}ms; ` +
        `tree CPU median=${result.streamingTree.cpuPercentMedian.toFixed(1)}% max=${result.streamingTree.cpuPercentMax.toFixed(1)}%; ` +
        `tree workingSet median=${result.streamingTree.workingSetMiBMedian.toFixed(1)}MiB; ` +
        `electron CPU median=${result.streaming.cpuPercentMedian.toFixed(1)}%`,
    );

    // 只断言“4 路都真的流式起来且首段可测”，数值本身落盘供趋势对比，不做阈值判定。
    expect(lanes).toHaveLength(LANE_COUNT);
    expect(result.firstSegmentMs.max).toBeGreaterThan(0);
    expect(streamingTreeSamples.length).toBeGreaterThan(2);
  });
});
