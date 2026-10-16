// @vitest-environment jsdom

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import {
  ChatContextUsage,
  buildContextUsageBreakdownSegments,
  buildContextUsageProgressSegments,
  formatContextCacheHitRateLabel,
  formatContextUsageSummary,
  formatContextUsageTokenCount,
} from "../src/chat-input-toolbar/contextUsage.js";

// Context 面板在真实组件里走 Radix HoverCard Portal；静态 markup 断言只关心内容，
// 这里把弹层骨架替换成透传元素，绕开 Portal 的挂载语义。
vi.mock("@/components/ai-elements/context.js", async () => {
  const React = await import("react");
  const passthrough =
    (tag: "button" | "div") =>
    ({ children, ...props }: { children?: React.ReactNode }) =>
      React.createElement(tag, props, children);

  return {
    Context: passthrough("div"),
    ContextContent: passthrough("div"),
    ContextContentBody: passthrough("div"),
    ContextTrigger: passthrough("button"),
  };
});

describe("buildContextUsageBreakdownSegments", () => {
  it("aggregates valid chars by source and sorts largest first", () => {
    expect(
      buildContextUsageBreakdownSegments([
        { source: "messages", chars: 700 },
        { source: "system_tool_schemas", chars: 200 },
        { source: "messages", chars: 100 },
        { source: "skills", chars: 0 },
      ]),
    ).toEqual([
      { source: "messages", chars: 800, percent: 0.8 },
      { source: "system_tool_schemas", chars: 200, percent: 0.2 },
    ]);
  });

  it("returns an empty list when no positive chars are available", () => {
    expect(
      buildContextUsageBreakdownSegments([
        { source: "messages", chars: 0 },
        { source: "skills", chars: -1 },
      ]),
    ).toEqual([]);
  });

  it("assigns the usage chart tone ramp to the largest segment first", () => {
    const segments = buildContextUsageBreakdownSegments([
      { source: "messages", chars: 100 },
      { source: "mcp_tool_schemas", chars: 900 },
      { source: "system_prompt", chars: 300 },
    ]);

    expect(buildContextUsageProgressSegments(segments)).toEqual([
      {
        id: "mcp_tool_schemas",
        percent: 0.6923076923076923,
        style: { backgroundColor: "var(--color-usage-chart-1)" },
      },
      {
        id: "system_prompt",
        percent: 0.23076923076923078,
        style: {
          backgroundColor:
            "color-mix(in oklab, var(--color-usage-chart-1) 78%, var(--color-surface))",
        },
      },
      {
        id: "messages",
        percent: 0.07692307692307693,
        style: {
          backgroundColor:
            "color-mix(in oklab, var(--color-usage-chart-1) 58%, var(--color-surface))",
        },
      },
    ]);
  });

  it("keeps later usage chart tones readable on the popover background", () => {
    const segments = buildContextUsageBreakdownSegments([
      { source: "messages", chars: 600 },
      { source: "system_prompt", chars: 500 },
      { source: "meta_user_context", chars: 400 },
      { source: "skills", chars: 300 },
      { source: "tool_prompt", chars: 200 },
      { source: "system_tool_schemas", chars: 100 },
    ]);

    expect(buildContextUsageProgressSegments(segments).at(-1)?.style).toEqual({
      backgroundColor: "color-mix(in oklab, var(--color-usage-chart-1) 28%, var(--color-surface))",
    });
  });
});

describe("formatContextUsageTokenCount", () => {
  it("uses uppercase compact suffixes for English context usage", () => {
    expect(formatContextUsageTokenCount(399_900, "en-US")).toBe("399.9K");
    expect(formatContextUsageTokenCount(200_000, "en-US")).toBe("200K");
    expect(formatContextUsageTokenCount(1_250_000, "en-US")).toBe("1.3M");
  });

  it("uses localized compact units for Chinese context usage", () => {
    expect(formatContextUsageTokenCount(399_900, "zh-CN")).toBe("40万");
    expect(formatContextUsageTokenCount(200_000, "zh-CN")).toBe("20万");
    expect(formatContextUsageTokenCount(120_000_000, "zh-CN")).toBe("1.2亿");
  });
});

describe("formatContextUsageSummary", () => {
  it("combines compact token values and usage percentage", () => {
    expect(
      formatContextUsageSummary({
        locale: "en-US",
        percent: 1,
        size: 200_000,
        used: 399_900,
      }),
    ).toBe("399.9K/200K (100%)");
  });
});

describe("formatContextCacheHitRateLabel", () => {
  it("keeps the production threshold for low cache hit rates", () => {
    expect(formatContextCacheHitRateLabel(0.048, "en-US")).toBeNull();
  });

  it("formats low cache hit rates when development diagnostics are enabled", () => {
    expect(
      formatContextCacheHitRateLabel(0.048, "en-US", {
        showBelowThreshold: true,
      }),
    ).toBe("4.8%");
  });

  it("does not invent a cache hit rate when the provider omits it", () => {
    expect(
      formatContextCacheHitRateLabel(null, "en-US", {
        showBelowThreshold: true,
      }),
    ).toBeNull();
  });
});

describe("ChatContextUsage", () => {
  it("renders nothing without renderable task usage", () => {
    const html = renderToStaticMarkup(
      createElement(ChatContextUsage, {
        intl: {
          formatMessage: ({ id }: { id: string }) => id,
        },
        locale: "en-US",
        selectedProvider: "glm",
        taskUsage: null,
      }),
    );

    expect(html).not.toContain("chat.contextUsage.title");
    expect(html).not.toContain("0/1M (0%)");
  });

  it("renders cache hit rate when the context usage reaches the display threshold", () => {
    const html = renderToStaticMarkup(
      createElement(ChatContextUsage, {
        intl: {
          formatMessage: ({ id }: { id: string }) => id,
        },
        locale: "en-US",
        selectedProvider: "glm",
        taskUsage: {
          used: 17_000,
          size: 1_000_000,
          cache: { hitRate: 0.8 },
        },
      }),
    );

    expect(html).toContain("chat.contextUsage.cacheHitRate");
    expect(html).toContain("80%");
  });

  it("renders a low cache hit rate in the development environment", () => {
    const html = renderToStaticMarkup(
      createElement(ChatContextUsage, {
        intl: {
          formatMessage: ({ id }: { id: string }) => id,
        },
        locale: "en-US",
        selectedProvider: "glm",
        taskUsage: {
          used: 17_000,
          size: 1_000_000,
          cache: { hitRate: 0.048 },
        },
      }),
    );

    expect(import.meta.env.DEV).toBe(true);
    expect(html).toContain("chat.contextUsage.cacheHitRate");
    expect(html).toContain("4.8%");
  });

  it("does not render per-source token counts in the breakdown rows", () => {
    const html = renderToStaticMarkup(
      createElement(ChatContextUsage, {
        intl: {
          formatMessage: ({ id }: { id: string }) => id,
        },
        locale: "en-US",
        selectedProvider: "glm",
        taskUsage: {
          used: 17_000,
          size: 1_000_000,
          breakdown: [
            { source: "system_tool_schemas", chars: 52_000 },
            { source: "messages", chars: 38_000 },
          ],
        },
      }),
    );

    expect(html).toContain("chat.contextUsage.breakdown.systemTools");
    expect(html).toContain("chat.contextUsage.breakdown.messages");
    expect(html).toContain("57.8%");
    expect(html).toContain("42.2%");
    expect(html).not.toContain("52K");
    expect(html).not.toContain("38K");
  });
});
