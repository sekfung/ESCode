import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { TID_V4_TIMELINE } from "@escode/shared";
import {
  prepareV4ConversationE2E,
  sendV4Prompt,
  waitForV4AssistantMessageContaining,
} from "../helpers/v4-conversation.js";

// App 级「首条消息第一段」精确测量（docs/specs/rust-unified-host.md §5）。
//
// 为什么单独一个用例：`waitForV4AssistantMessageContaining` 走 WebDriver 轮询（默认 500ms），
// 用它计时会把粒度误差算进首段（实测 1.3–1.5 s，明显高于真实值）。这里改为应用内计时：
// 在渲染进程安装 MutationObserver，记录「提交前 performance.now()」与「首个 assistant 行首次
// 出现非空文本的 performance.now()」，两者相减。
//
// 负载：fixture `E2E_APP_PERF_FIRST_<n>` 的首个文本 delta 在 0–1ms，因此测得的间隔≈
// 提交 → IPC/引擎投影 → 首字渲染的 App 侧开销（不含真实模型首字延迟）。
//
// 前提：replay fixture 包含 common.json 与 perf/app-perf-streaming.json。

const SAMPLES = 5;

interface FirstSegmentProbe {
  installedAt: number;
  submitAt: number;
  firstText: number;
  firstTextLength: number;
}

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

describe("APP-PERF: 首条消息第一段（应用内计时）", () => {
  it("测量提交到首个 assistant 文本渲染的间隔", async function () {
    this.timeout(10 * 60_000);
    await prepareV4ConversationE2E();

    const timelineTestId = TID_V4_TIMELINE;
    const startProbe = async (): Promise<number> =>
      browser.execute((testId) => {
        const target = {
          installedAt: 0,
          submitAt: 0,
          firstText: 0,
          firstTextLength: 0,
        };
        (window as Window & { __appPerfFirstSegment?: typeof target }).__appPerfFirstSegment =
          target;
        target.installedAt = performance.now();
        // 起点必须是真实 submit 事件：若从 browser.execute 调用前开始计时，会把
        // setV4ComposerText / 发送按钮轮询等多次 WebDriver 往返算进首段（实测虚高约 300ms）。
        document.addEventListener(
          "submit",
          () => {
            if (target.submitAt === 0) target.submitAt = performance.now();
          },
          { capture: true, once: true },
        );

        const readAssistantText = () => {
          const timeline = document.querySelector<HTMLElement>(`[data-testid="${testId}"]`);
          if (!timeline) return "";
          return Array.from(timeline.querySelectorAll<HTMLElement>("[data-row-id]"))
            .filter((row) => row.classList.contains("group/assistant-row"))
            .map((row) => row.innerText.trim())
            .join("");
        };

        // 同一会话里上一轮 assistant 行还在：以基线长度为起点，只有文本继续增长才算本轮首字。
        const baselineLength = readAssistantText().length;
        const observer = new MutationObserver(() => {
          if (target.firstText !== 0) return;
          const text = readAssistantText();
          if (text.length <= baselineLength) return;
          target.firstText = performance.now();
          target.firstTextLength = text.length;
          observer.disconnect();
        });
        observer.observe(document.body, {
          childList: true,
          characterData: true,
          subtree: true,
        });
        return target.installedAt;
      }, timelineTestId);

    const readProbe = async (): Promise<FirstSegmentProbe> =>
      browser.execute(() => {
        const probe = (
          window as Window & {
            __appPerfFirstSegment?: {
              installedAt: number;
              submitAt: number;
              firstText: number;
              firstTextLength: number;
            };
          }
        ).__appPerfFirstSegment;
        if (!probe) throw new Error("first-segment probe missing");
        return { ...probe };
      });

    const samples: number[] = [];
    for (let lane = 1; lane <= SAMPLES; lane++) {
      await startProbe();
      await sendV4Prompt(`E2E_APP_PERF_FIRST_${lane}`);
      await waitForV4AssistantMessageContaining(`PERF_FIRST_${lane}_START`, 120_000);
      const probe = await readProbe();
      if (probe.submitAt === 0 || probe.firstText <= probe.submitAt) {
        throw new Error(
          `first-segment probe did not observe submit/text: ${JSON.stringify(probe)}`,
        );
      }
      samples.push(probe.firstText - probe.submitAt);
      // 让上一轮收尾，避免下一轮命中旧行。
      await waitForV4AssistantMessageContaining(`PERF_FIRST_${lane}_DONE`, 120_000);
      await sleep(200);
    }

    const runtimeLabel = process.env.ESCODE_APP_PERF_LABEL?.trim() || "default";
    const result = {
      kind: "app-perf-first-segment",
      runtimeLabel,
      platform: `${process.platform}-${process.arch}`,
      recordedAt: new Date().toISOString(),
      sampleCount: samples.length,
      fixtureFirstTextOffsetMs: 1,
      // 应用内计时：真实 submit 事件 → 首个 assistant 文本渲染。
      firstSegmentMs: {
        values: samples,
        median: median(samples),
        max: Math.max(...samples),
      },
    };
    const file = join(
      artifactDir(),
      `app-perf-first-segment.${runtimeLabel}.${process.platform}-${process.arch}.json`,
    );
    writeFileSync(file, JSON.stringify(result, null, 2) + "\n");
    console.log(`[app-perf] ${file}`);
    console.log(
      `[app-perf] first segment (in-app) median=${result.firstSegmentMs.median.toFixed(1)}ms ` +
        `max=${result.firstSegmentMs.max.toFixed(1)}ms values=[${samples
          .map((value) => value.toFixed(1))
          .join(", ")}]`,
    );

    expect(samples).toHaveLength(SAMPLES);
    expect(result.firstSegmentMs.median).toBeGreaterThan(0);
  });
});
