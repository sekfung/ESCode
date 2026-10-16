# Chat Input Context

> **当前状态**：本文已按 V4 composer 接线校准。上下文用量来自 V4 conversation snapshot，
> 不再读取 legacy task runtime/store。套餐额度区块已随套餐族删除（2026-10-15），本文只描述上下文用量部分。

## 目标

把聊天输入栏底部的上下文用量，从静态文本改成 AI Elements 的 `Context` compound component。

## 当前实现

- `packages/ui/src/v4/composer/V4ComposerToolbar.tsx`
  - 把 snapshot 的 `usage.contextWindow.usedTokens` / `maxTokens` / cache / breakdown 转成输入栏展示模型
  - 复用 `packages/ui/src/chat-input-toolbar/contextUsage.tsx` 的 `ChatContextUsage`
  - 使用 `ContextTrigger` 作为上下文入口
  - 使用 `ContextContentBody` 展示上下文窗口占用、breakdown 与 cache hit rate
  - cache hit rate 只有在主轮累计命中率大于等于 78% 时展示；低于 78% 的值不在紧凑 context 面板中显示
- `packages/ui/src/components/ai-elements/context.tsx`
  - 增加了 0 上限保护，避免进度百分比出现无效值
  - 改为跟随当前环境 locale 格式化百分比、compact 数字和货币
- 展示细节
  - `Context windows` 标题使用 `text-ui-base`
  - `Context windows` 标题右侧的 used / total 摘要使用本地化紧凑 token 单位；英文使用大写 `K` / `M` / `B`，中文使用 `万` / `亿`
  - `Context windows` 进度分段色阶以 `--color-usage-chart-1` 为基准，按占比顺序逐级变浅
- `packages/ui/src/i18n/locales/en-US.ts`
- `packages/ui/src/i18n/locales/zh-CN.ts`
  - 补充了 hover card 的说明文案

## 结果

- 输入栏底部仍保留简洁的 context 入口
- 用户悬停后可以看到更完整的上下文解释
- 组件形式与 `elements.ai-sdk` 的 context 文档保持一致
