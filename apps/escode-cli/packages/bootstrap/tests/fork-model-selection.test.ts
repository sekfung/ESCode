// 会话派生（分叉 / 编辑前分叉）的常驻 Selection 推导回归。
// 推导规则：优先继承分叉点消息上的选型（只回落推理档位），消息没有选型事实时
// 退回父会话常驻 Selection。
import { describe, expect, it } from "vitest";
import {
  cloneModelSelection,
  derivedSessionModelSelection,
  modelSelectionWithOptionFallback,
} from "../src/escode-protocol/fork-model-selection.js";

const sessionSelection = {
  providerId: "builtin:zai-coding-plan",
  modelId: "glm-4.6",
  options: { reasoningLevel: "high" },
};

describe("派生会话的常驻 Selection", () => {
  it("普通分叉点继承消息选型，缺失推理档位时回落会话档位", () => {
    expect(
      derivedSessionModelSelection(
        { providerId: "provider-a", modelId: "model-a", options: { reasoningLevel: "low" } },
        sessionSelection,
      ),
    ).toEqual({
      providerId: "provider-a",
      modelId: "model-a",
      options: { reasoningLevel: "low" },
    });
    expect(
      derivedSessionModelSelection(
        { providerId: "provider-a", modelId: "model-a" },
        sessionSelection,
      ),
    ).toEqual({
      providerId: "provider-a",
      modelId: "model-a",
      options: { reasoningLevel: "high" },
    });
  });

  it("分叉点消息没有选型事实时按会话常驻 Selection 派生", () => {
    const derived = derivedSessionModelSelection(undefined, sessionSelection);
    expect(derived).toEqual(sessionSelection);
    // 派生结果是副本，不与父会话的常驻 Selection 共享 options 引用。
    expect(derived).not.toBe(sessionSelection);
    expect(derivedSessionModelSelection(undefined, undefined)).toBeUndefined();
  });
});

describe("Selection 复制原语", () => {
  it("cloneModelSelection 不与源共享 options 引用", () => {
    const cloned = cloneModelSelection(sessionSelection);
    expect(cloned).toEqual(sessionSelection);
    expect(cloned?.options).not.toBe(sessionSelection.options);
    expect(cloneModelSelection(undefined)).toBeUndefined();
  });

  it("modelSelectionWithOptionFallback 只回落推理档位，不回落模型身份", () => {
    expect(
      modelSelectionWithOptionFallback({ providerId: "p", modelId: "m" }, sessionSelection),
    ).toEqual({ providerId: "p", modelId: "m", options: { reasoningLevel: "high" } });
    expect(modelSelectionWithOptionFallback({ providerId: "p", modelId: "m" }, undefined)).toEqual({
      providerId: "p",
      modelId: "m",
    });
  });
});
