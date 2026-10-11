import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  appendCircuitDraftReference,
  buildPromptWithCircuitReferences,
  clearCircuitDraftReferences,
  dispatchCircuitDraftReference,
  getCircuitDraftReferenceEventName,
  getCircuitDraftReferences,
  isCircuitDraftReferenceEvent,
  parsePromptCircuitReferences,
  removeCircuitDraftReference,
  setCircuitDraftReferences,
} from "@/lib/circuit/circuitReferenceDrafts.js";
import type { CircuitDraftReference } from "@/circuit/domain/circuitReferences.js";

/** 电路引用 chip 的 renderer 草稿与 prompt 尾块（规格 §8：chip 只属于草稿，不自动发送）。 */

const chip = (name: string, kind: "component" | "net" = "component"): CircuitDraftReference => ({
  irId: "ir:1",
  kind,
  name,
  label: kind === "net" ? `NET:${name}` : name,
  detail: `${name} (component uuid-${name})`,
});

describe("电路引用草稿", () => {
  beforeEach(() => {
    vi.stubGlobal("window", new EventTarget());
    clearCircuitDraftReferences("/workspace");
  });
  afterEach(() => {
    clearCircuitDraftReferences("/workspace");
    vi.unstubAllGlobals();
  });

  it("按 workspace 隔离，插入同一个元素只保留一条", () => {
    setCircuitDraftReferences("/workspace", [chip("U3")]);
    expect(getCircuitDraftReferences("/workspace")).toHaveLength(1);
    expect(getCircuitDraftReferences("/other")).toEqual([]);
    expect(appendCircuitDraftReference(getCircuitDraftReferences("/workspace"), chip("U3"))).toHaveLength(1);
    expect(appendCircuitDraftReference(getCircuitDraftReferences("/workspace"), chip("U4"))).toHaveLength(2);
  });

  it("dispatch 后草稿可见，并带事件通知 composer", () => {
    const listener = vi.fn();
    window.addEventListener(getCircuitDraftReferenceEventName(), listener);
    dispatchCircuitDraftReference("/workspace", chip("I2C1_SDA", "net"));
    expect(getCircuitDraftReferences("/workspace")).toMatchObject([{ label: "NET:I2C1_SDA" }]);
    expect(listener).toHaveBeenCalledTimes(1);
    const event = listener.mock.calls[0]?.[0] as Event;
    expect(isCircuitDraftReferenceEvent(event)).toBe(true);
    expect((event as CustomEvent<{ workspaceKey: string }>).detail.workspaceKey).toBe("/workspace");
  });

  it("删除单条引用后草稿里不再出现该元素", () => {
    setCircuitDraftReferences("/workspace", [chip("U3"), chip("R1")]);
    expect(removeCircuitDraftReference("/workspace", chip("U3"))).toMatchObject([{ name: "R1" }]);
    expect(getCircuitDraftReferences("/workspace")).toMatchObject([{ name: "R1" }]);
  });
});

describe("电路引用 prompt 尾块", () => {
  it("序列化后能从历史正文里解析回同样的引用，并剥掉尾块", () => {
    const text = "看看这个电路";
    const prompt = buildPromptWithCircuitReferences(text, [chip("U3"), chip("I2C1_SDA", "net")]);
    expect(prompt.startsWith(text)).toBe(true);
    const parsed = parsePromptCircuitReferences(prompt);
    expect(parsed.visibleContent).toBe(text);
    expect(parsed.circuitReferences).toMatchObject([
      { irId: "ir:1", kind: "component", label: "U3" },
      { irId: "ir:1", kind: "net", label: "NET:I2C1_SDA" },
    ]);
  });

  it("没有引用时不改正文", () => {
    expect(buildPromptWithCircuitReferences("原文", [])).toBe("原文");
    expect(parsePromptCircuitReferences("原文")).toEqual({
      visibleContent: "原文",
      circuitReferences: [],
    });
  });

  it("尾块结构不合法时整块不识别，正文原样保留", () => {
    const broken = `正文\n\n# Circuit references:\n\n用户选择\n\n\`\`\`circuit-references\n{"irId":"ir:1"}\n\`\`\``;
    expect(parsePromptCircuitReferences(broken)).toEqual({
      visibleContent: broken,
      circuitReferences: [],
    });
  });
});
