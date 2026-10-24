import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppErrorBoundary, ScopedErrorBoundary } from "@/ErrorBoundary.js";

afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("AppErrorBoundary", () => {
  it("出错后会渲染 fallback，而不是返回空白内容", () => {
    const boundary = new AppErrorBoundary({
      children: createElement("div", null, "ok"),
    });

    boundary.state = {
      error: new Error("boom"),
      componentStack: "\n    at BrokenPanel",
    };

    const html = renderToStaticMarkup(boundary.render());

    expect(html).toMatch(/应用界面出了点问题|The app ran into a problem/);
    expect(html).toMatch(/重试|Try again/);
    expect(html).toMatch(/刷新应用|Reload app/);
    expect(html).toContain("boom");
    expect(html).toContain("BrokenPanel");
  });

  it("会把非 Error 抛出值包装成 Error 对象", () => {
    const state = AppErrorBoundary.getDerivedStateFromError("plain text failure");

    expect(state.error?.message).toBe("plain text failure");
    expect(state.componentStack).toBe("");
  });

  it("局部边界出错后只渲染区域 fallback", () => {
    const boundary = new ScopedErrorBoundary({
      children: createElement("div", null, "ok"),
      scope: "workspace-chat",
      resetKeys: ["workspace-a", "task-a"],
    });

    boundary.state = {
      error: new Error("chat exploded"),
      componentStack: "\n    at ChatView",
    };

    const html = renderToStaticMarkup(boundary.render());

    expect(html).toMatch(/这块界面出了点问题|This section ran into a problem/);
    expect(html).toMatch(/重试此区域|Try this section again/);
    expect(html).toContain("chat exploded");
    expect(html).toContain("ChatView");
  });

  it("静默局部边界可以隐藏非关键浮层 fallback", () => {
    const boundary = new ScopedErrorBoundary({
      children: createElement("div", null, "ok"),
      scope: "desktop-top-overlay",
      variant: "silent",
    });

    boundary.state = {
      error: new Error("overlay exploded"),
      componentStack: "\n    at DesktopTopOverlay",
    };

    expect(renderToStaticMarkup(boundary.render())).toBe("");
  });

  it("生产构建下把崩溃根因写入桌面日志桥", () => {
    // 修复原因：生产构建里普通 logger.error 整体 no-op，导出的现场日志只剩 componentStack、
    // 没有 error.message；错误边界崩溃是低频事件，必须走 lifecycle 诊断通道留下根因。
    vi.stubEnv("PROD", true);
    const bridgeLog = vi.fn();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("window", { escode: { log: bridgeLog } });

    const boundary = new ScopedErrorBoundary({
      children: createElement("div", null, "ok"),
      scope: "workspace-chat",
    });
    vi.spyOn(boundary, "setState").mockImplementation(() => {});
    boundary.componentDidCatch(new Error("Objects are not valid as a React child"), {
      componentStack: "\n    at button",
    });

    expect(consoleError).not.toHaveBeenCalled();
    expect(bridgeLog).toHaveBeenCalledWith("error", [
      "[ScopedErrorBoundary:workspace-chat] React subtree crashed:",
      expect.objectContaining({ message: "Objects are not valid as a React child" }),
      "\n    at button",
    ]);
  });
});
