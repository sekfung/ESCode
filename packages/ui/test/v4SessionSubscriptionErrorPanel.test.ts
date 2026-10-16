// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ESCodeIntlProvider } from "@/i18n/IntlProvider.js";
import { SessionSubscriptionErrorPanel } from "@/v4/SessionSubscriptionErrorPanel.js";

function renderPanel() {
  return render(
    createElement(
      ESCodeIntlProvider,
      { initialLocale: "zh-CN" },
      createElement(SessionSubscriptionErrorPanel, {
        error: "EPERM: operation not permitted",
        onReconnect: vi.fn(),
      }),
    ),
  );
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("SessionSubscriptionErrorPanel", () => {
  it("shows the subscription error with the reconnect action", () => {
    renderPanel();

    expect(screen.getByText("EPERM: operation not permitted")).toBeTruthy();
    expect(screen.getByRole("button", { name: "重新连接" })).toBeTruthy();
  });
});
