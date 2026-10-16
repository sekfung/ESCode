// @vitest-environment jsdom
import { createElement } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { DesktopCommandIds } from "@escode/shared";
import { EmbeddedWebsiteHeader } from "@/components/EmbeddedWebsiteHeader.js";

const { executeDesktopCommand } = vi.hoisted(() => ({
  executeDesktopCommand: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/hooks/usePlatform.js", () => ({
  usePlatform: () => ({ executeDesktopCommand }),
}));
vi.mock("@/i18n/IntlProvider.js", () => ({
  useESCodeIntl: () => ({
    intl: { formatMessage: ({ id }: { id: string }) => id },
  }),
}));
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

// 登录页与升级 webview 已随套餐族/账号登录下线；此处保留内嵌 webview 外壳的
// 覆盖层窗控契约：网页关闭与窗口关闭是两条独立链路。
it("内嵌 webview 的网页关闭与窗口关闭分开，三种窗口操作走现有命令", () => {
  const onClose = vi.fn();
  render(
    createElement(EmbeddedWebsiteHeader, {
      title: "Embedded",
      loading: false,
      canGoBack: false,
      canGoForward: false,
      onBack: vi.fn(),
      onForward: vi.fn(),
      onReload: vi.fn(),
      onClose,
    }),
  );
  fireEvent.click(screen.getByRole("button", { name: "common.close" }));
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(executeDesktopCommand).not.toHaveBeenCalled();
  for (const [id, command] of [
    ["minimize", DesktopCommandIds.MinimizeWindow],
    ["maximize", DesktopCommandIds.ToggleMaximizeWindow],
    ["close", DesktopCommandIds.CloseWindow],
  ]) {
    fireEvent.click(screen.getByTestId(`window-control-${id}`));
    expect(executeDesktopCommand).toHaveBeenLastCalledWith(command);
  }
  expect(onClose).toHaveBeenCalledTimes(1);
});
