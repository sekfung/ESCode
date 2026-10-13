// @vitest-environment jsdom
import { createElement } from "react";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import zhCN from "@/i18n/locales/zh-CN.js";
import { projectCircuitReferences } from "@/circuit/domain/circuitReferences.js";
import { CircuitMessageResponse } from "@/circuit/components/CircuitMessageResponse.js";

/**
 * `::circuit` 引用卡片在非 Desktop 的行为（docs/circuit/README.md §8、§3.5、feature 第 7 条）。
 * Web / 手机没有 CircuitService：`::circuit` 不渲染任何内容，不加载电路面板、不订阅 circuit.changed、
 * 也不向服务发起任何请求。
 * 桌面 E2E（wdio）只覆盖 Desktop 客户端，这里的平台能力用 mock 固定。
 */

const h = vi.hoisted(() => ({
  platform: { supportsCircuit: undefined as boolean | undefined },
  circuitService: {
    correct: vi.fn(),
    get: vi.fn(),
    listCorrections: vi.fn(),
    onChanged: vi.fn(),
    parse: vi.fn(),
    query: vi.fn(),
  },
}));

vi.mock("@/hooks/usePlatform.js", () => ({ usePlatform: () => h.platform }));
vi.mock("@/hooks/useServices.js", () => ({
  useOptionalServices: () => ({ circuitService: h.circuitService }),
}));
vi.mock("@/i18n/IntlProvider.js", () => ({
  useESCodeIntl: () => ({
    intl: {
      // 直接用 zh-CN 文案做替换，断言比对真实用户可见文本而不是 message id。
      formatMessage: (descriptor: { id: string }, values?: Record<string, string | number>) => {
        let text = zhCN[descriptor.id] ?? descriptor.id;
        for (const [key, value] of Object.entries(values ?? {})) {
          text = text.replace(`{${key}}`, String(value));
        }
        return text;
      },
    },
  }),
}));

const REFERENCE_TEXT = '::circuit{"irId":"ir:rp2040-minimal","title":"RP2040_minimal"}';

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  h.platform.supportsCircuit = undefined;
});

it("Web 客户端不渲染 ::circuit 卡片，不加载面板也不订阅事件", () => {
  render(
    createElement(CircuitMessageResponse, {
      children: REFERENCE_TEXT,
      completed: true,
      messageKey: "e2e-web-card",
    }),
  );

  // 非 Desktop 不渲染任何卡片：也没有「打开面板」入口，也不读服务、不订阅事件。
  expect(screen.queryByTestId("circuit-card")).toBeNull();
  expect(screen.queryByTestId("circuit-card-open")).toBeNull();
  expect(screen.queryByTestId("circuit-panel")).toBeNull();
  expect(h.circuitService.get).not.toHaveBeenCalled();
  expect(h.circuitService.onChanged).not.toHaveBeenCalled();
  expect(h.circuitService.correct).not.toHaveBeenCalled();
});

it("代码块（围栏内）里的 ::circuit 保持正文原样，不渲染引用卡片", () => {
  const fenced = `\`\`\`text\n${REFERENCE_TEXT}\n\`\`\``;
  expect(projectCircuitReferences(fenced, true)).toEqual([{ kind: "text", text: fenced }]);
});

it("Desktop（有能力位）时卡片提供打开面板入口，并由服务解析出源文件路径", async () => {
  h.platform.supportsCircuit = true;
  h.circuitService.get.mockResolvedValue({
    summary: { source: { path: "hardware/RP2040_minimal.kicad_sch" } },
  });
  const onOpenCodeViewer = vi.fn();
  render(
    createElement(CircuitMessageResponse, {
      children: REFERENCE_TEXT,
      completed: true,
      messageKey: "e2e-desktop-card",
      onOpenCodeViewer,
      workspacePath: "/tmp/escode-workspace",
    }),
  );

  expect(screen.getByTestId("circuit-card").getAttribute("data-circuit-card-mode")).toBe("desktop");
  screen.getByTestId("circuit-card-open").click();
  await vi.waitFor(() => expect(onOpenCodeViewer).toHaveBeenCalledTimes(1));
  expect(h.circuitService.get).toHaveBeenCalledWith({
    workspacePath: "/tmp/escode-workspace",
    irId: "ir:rp2040-minimal",
  });
  expect(onOpenCodeViewer.mock.calls[0]?.[0]).toMatchObject({
    circuitPanel: { irId: "ir:rp2040-minimal" },
    path: "hardware/RP2040_minimal.kicad_sch",
    workspacePath: "/tmp/escode-workspace",
  });
});

it("面板「插入到输入框」派发的事件把引用写进对应 workspace 的 composer 草稿", async () => {
  const { useCircuitDraftReferences } = await import("@/hooks/useCircuitDraftReferences.js");
  const { dispatchCircuitDraftReference } = await import("@/lib/circuit/circuitReferenceDrafts.js");
  function Probe() {
    const { references } = useCircuitDraftReferences("/tmp/ws");
    return createElement(
      "div",
      { "data-testid": "probe-references" },
      references.map((reference) => reference.label).join(","),
    );
  }
  render(createElement(Probe));
  expect(screen.getByTestId("probe-references").textContent).toBe("");

  act(() => {
    dispatchCircuitDraftReference("/tmp/ws", {
      detail: "U1 (component uuid-u1)",
      irId: "ir:rp2040-minimal",
      kind: "component",
      label: "U1",
      name: "U1",
    });
  });
  expect(screen.getByTestId("probe-references").textContent).toBe("U1");

  // 其它 workspace 的事件不会串进当前 composer（草稿按 workspace 隔离）。
  act(() => {
    dispatchCircuitDraftReference("/tmp/other", {
      detail: "NET:+3V3 (net net:+3V3)",
      irId: "ir:rp2040-minimal",
      kind: "net",
      label: "NET:+3V3",
      name: "+3V3",
    });
  });
  expect(screen.getByTestId("probe-references").textContent).toBe("U1");
});
