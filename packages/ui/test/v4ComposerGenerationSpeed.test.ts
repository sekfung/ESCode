// @vitest-environment jsdom

import { createElement } from "react";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ESCodeIntlProvider } from "@/i18n/IntlProvider.js";
import { V4ComposerGenerationSpeed } from "@/v4/composer/V4ComposerGenerationSpeed.js";
import {
  LocalGenerationSpeedTracker,
  setLocalGenerationSpeedTracker,
} from "@/v4/telemetry/localGenerationSpeed.js";
import type { ConversationTopicFrame } from "@escode/shared/escode-protocol-v4";

// 行语义、窗口口径与缺口语义在 localGenerationSpeed.test.ts 覆盖；这里只验证读数的呈现契约
// （格式、占位符、无障碍标签与 data-* 锚点）。

const SESSION_ID = "session-speed-ui";
const clock = { now: 0 };

function online(deltas: unknown[]): ConversationTopicFrame {
  return {
    topic: `conversation/${SESSION_ID}`,
    subscriptionId: "sub-ui",
    fromSeq: 0,
    toSeq: 0,
    sentAt: clock.now,
    payload: { kind: "deltas", deltas },
  } as unknown as ConversationTopicFrame;
}

function row(kind: string, rowId: number, extra: Record<string, unknown> = {}) {
  return {
    op: "row.appended",
    row: { rowId, turnId: "turn-ui", createdAt: 0, createdAtSeq: rowId, kind, ...extra },
  };
}

function newTracker() {
  const tracker = new LocalGenerationSpeedTracker(() => clock.now);
  setLocalGenerationSpeedTracker(tracker);
  clock.now = 0;
  return tracker;
}

function beginTurn(tracker: LocalGenerationSpeedTracker) {
  tracker.receive(
    online([row("turnHeader", 1, { origin: "userInput", state: "running", startedAt: 0 })]),
    "online",
  );
  tracker.receive(online([row("assistantText", 2, { text: "", state: "streaming" })]), "online");
}

/** 读数用本地追踪器状态渲染；断言取 DOM 属性与文案。 */
function renderReadout(locale: "zh-CN" | "en-US") {
  const { container, unmount } = render(
    createElement(
      ESCodeIntlProvider,
      { initialLocale: locale },
      createElement(V4ComposerGenerationSpeed, { sessionId: SESSION_ID }),
    ),
  );
  const element = container.querySelector<HTMLElement>("[data-generation-speed-mode]");
  if (!element) throw new Error("读数元素未渲染");
  return { element, unmount };
}

afterEach(() => {
  cleanup();
  setLocalGenerationSpeedTracker(undefined);
});

describe("V4ComposerGenerationSpeed", () => {
  it("无读数时显示占位符，不显示 0", () => {
    newTracker();
    const { element } = renderReadout("zh-CN");
    expect(element.textContent).toBe("—");
    expect(element.getAttribute("data-generation-speed-mode")).toBe("none");
    expect(element.getAttribute("data-generation-speed-value")).toBe("");
    expect(element.getAttribute("aria-label")).toBe("生成速度暂无读数");
  });

  it("实时值按整数显示并标注为实时口径", () => {
    const tracker = newTracker();
    beginTurn(tracker);
    tracker.receive(
      online([{ op: "row.delta", rowId: 2, path: "text", append: "abcdef" }]),
      "online",
    );
    clock.now = 1_000;
    tracker.receive(
      online([{ op: "row.delta", rowId: 2, path: "text", append: "abcdefghi" }]),
      "online",
    );

    const { element } = renderReadout("zh-CN");
    expect(element.textContent).toBe("5 tok/s");
    expect(element.getAttribute("data-generation-speed-mode")).toBe("live");
    expect(element.getAttribute("data-generation-speed-value")).toBe("5");
    expect(element.getAttribute("aria-label")).toContain("实时生成速度");
    // 规格 2.3：实时态用次要色，保留态才降到最弱色。
    expect(element.className).toContain("text-foreground-subtle");
    expect(element.className).not.toContain("text-foreground-subtlest");
  });

  it("小于 1 时保留一位小数", () => {
    const tracker = newTracker();
    beginTurn(tracker);
    tracker.receive(online([{ op: "row.delta", rowId: 2, path: "text", append: "abc" }]), "online");
    clock.now = 2_500;
    tracker.receive(online([{ op: "row.delta", rowId: 2, path: "text", append: "abc" }]), "online");

    const { element } = renderReadout("zh-CN");
    expect(element.textContent).toBe("0.8 tok/s");
    expect(element.getAttribute("data-generation-speed-value")).toBe("0.8");
  });

  it("轮末保留解码速度并弱化显示", () => {
    const tracker = newTracker();
    beginTurn(tracker);
    tracker.receive(
      online([{ op: "row.delta", rowId: 2, path: "text", append: "abcdef" }]),
      "online",
    );
    clock.now = 1_000;
    tracker.receive(
      online([{ op: "row.delta", rowId: 2, path: "text", append: "abcdefghi" }]),
      "online",
    );

    clock.now = 2_000;
    tracker.receive(
      online([
        {
          op: "row.upserted",
          row: {
            rowId: 1,
            turnId: "turn-ui",
            createdAt: 0,
            createdAtSeq: 1,
            kind: "turnHeader",
            origin: "userInput",
            state: "completedSuccess",
            startedAt: 0,
            // 权威字段：2 秒解码区间 / 10 token → 5 tok/s。
            decodeMs: 2_000,
            decodeTokens: 10,
          },
        },
      ]),
      "online",
    );

    const { element } = renderReadout("zh-CN");
    expect(element.textContent).toBe("5 tok/s");
    expect(element.getAttribute("data-generation-speed-mode")).toBe("average");
    expect(element.getAttribute("aria-label")).toContain("上一轮解码速度");
    expect(element.className).toContain("text-foreground-subtlest");
  });

  it("英文界面使用英文无障碍标签与同一单位", () => {
    const tracker = newTracker();
    beginTurn(tracker);
    tracker.receive(
      online([{ op: "row.delta", rowId: 2, path: "text", append: "abcdef" }]),
      "online",
    );
    clock.now = 1_000;
    tracker.receive(
      online([{ op: "row.delta", rowId: 2, path: "text", append: "abcdefghi" }]),
      "online",
    );

    const { element } = renderReadout("en-US");
    expect(element.textContent).toBe("5 tok/s");
    expect(element.getAttribute("aria-label")).toContain("Live generation speed");
  });
});
