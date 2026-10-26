// @vitest-environment jsdom
// 回归：React 19 下 Radix Slot 必须给 clone 出来的 child 一个稳定 ref。
//
// 2026-10-26 生产日志（React #185）：打开会话时 asChild trigger 每 commit 都
// detach/attach 一次 ref。Radix Tooltip/Popper 把 state setter（onTriggerChange /
// onAnchorChange）作为 ref 传入，重新挂载 ref 会同步 setState → 再 render → 再
// detach，嵌套超过 50 次后整块 header/chat 被错误边界接管。
//
// Radix 上游在 @radix-ui/react-slot 里现场 composeRefs(forwardedRef, childrenRef)，
// 每次 render 都返回新函数。本仓库通过 pnpm patch 把它换成了记忆化的
// useComposedRefs；这里锁住「父组件重渲不得让 child ref 重新挂载」。
import { createElement, useCallback, useState } from "react";
import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Slot } from "radix-ui";

afterEach(cleanup);

describe("Radix Slot ref 稳定性（React #185）", () => {
  it("父组件重渲时 child ref 不得 detach/attach", () => {
    const calls: Array<"attach" | "detach"> = [];
    const stableRef = (node: HTMLElement | null) => {
      calls.push(node === null ? "detach" : "attach");
    };

    function Probe() {
      const [revision, setRevision] = useState(0);
      // 与 Radix asChild trigger 等价的组合方式：Slot 接收 ref 并克隆真实 child。
      const bump = useCallback(() => setRevision((value) => value + 1), []);
      return createElement(
        Slot.Root,
        { ref: stableRef },
        createElement("button", { type: "button", onClick: bump }, `bump ${revision}`),
      );
    }

    const { getByRole } = render(createElement(Probe));
    const button = getByRole("button");
    for (let index = 0; index < 5; index += 1) {
      fireEvent.click(button);
    }

    // 未打 patch 时 SlotClone 每次 render 都生成新的 composed ref：React 19 会先
    // detach(null) 再 attach(node)，calls 变成 attach/detach 交替。
    expect(calls).toEqual(["attach"]);
    expect(getByRole("button")).toBe(button);
  });
});
