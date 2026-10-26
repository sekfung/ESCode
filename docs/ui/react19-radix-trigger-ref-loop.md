# React 19 下 Radix asChild trigger 的 ref 抖动（React #185）

> 触发面：打开会话、会话数据首帧落位时。生产日志表现为
> `ScopedErrorBoundary:workspace-header` → `desktop-top-overlay` → `workspace-chat`
> 依次被 `Minified React error #185` 接管；`retry` 后再次复现。

## 根因

Radix 的 `asChild` trigger 走 `Slot` / `SlotClone`。这些实现（1.2.3 / 1.2.4
及同批 primitives）在 **render 期间现场组装 ref**：

```js
// @radix-ui/react-slot
props2.ref = forwardedRef ? composeRefs(forwardedRef, childrenRef) : childrenRef;

// @radix-ui/react-popper / dismissable-layer / focus-scope 等
useComposedRefs(forwardedRef, (node) => setNode(node));
```

`composeRefs(...)` 和箭头函数每次 render 都返回**新引用**。React 19 在 callback ref
身份变化时会先 `detach(null)` 再 `attach(node)`；而 Radix Tooltip / Popper 把
**state setter** 直接作为 ref 传入（`onTriggerChange` / `onAnchorChange`）。于是每个
commit 的 ref 重挂都会同步 `setState`，再触发下一次 render，形成自持的
detach/attach 循环；嵌套更新超过 50 次后 React 抛 #185。

会话首帧落位期间父级会连续重渲（虚拟列表测高、布局收敛），这个抖动被放大成整块
header/chat 被错误边界接管。它不在 `ConversationTimeline` 的滚动视口 setState 路径上，
所以 `37571efd` 的微任务化修复覆盖不到。

## 修复

1. `patches/@radix-ui__react-slot@1.2.3|1.2.4.patch`：`SlotClone` 改用记忆化的
   `useComposedRefs(forwardedRef, childrenRef)`，child ref 身份只在其输入变化时改变。
2. `patches/@radix-ui__react-{popper,dismissable-layer,focus-scope,menu,dropdown-menu,
radio-group,switch,scroll-area,slider,select,toast,navigation-menu}*.patch`：
   把现场箭头 / 现场 `composeRefs` 改为稳定的 setter 或 `useComposedRefs`。
3. `ControlHintTooltip`：调用方没有提供 `childRef` / `triggerRef` 时不再向 child 注入
   ref，避免多一层可变的 ref 来源。

## 验收与回归

- `packages/ui/test/radixSlotStableRef.test.ts`：父组件重渲 5 次后，Slot 的 child ref
  只允许 `attach` 一次；去掉 patch 时为 `attach/detach` 交替。
- 本地 dev 复现（合成 90 行会话）：修复前单次打开会话 App 渲染 ~333 次并出现 #185；
  修复后整页 30 次、打开会话 9 次，错误边界未触发，会话正文正常渲染。

## 维护

上游已在 `@radix-ui/react-slot` ≥ 1.2.5（`react-compose-refs` ≥ 1.1.3）及
`radix-ui` ≥ 1.6.2 修复同类问题。升级到这些版本后，应删除本组 patch 与
`patchedDependencies` 条目，并用上面的回归用例复核。
