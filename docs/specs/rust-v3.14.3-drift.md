# Rust runtime 对 v3.14.3 的漂移评估（WP1）

2026-09-24。Rust 分支基于 872ad96，main 在其后合入 v3.14.3（328c1a0，CLI/协议 142 个文件）。本文逐项判断 Rust 是否需要跟进；结论只基于源码 diff 与本机漂移检查，未跑 App 集成测试（磁盘不足）。

## 生成资产

| 资产                           | 结果                                                                           | 处理                                                                                                          |
| ------------------------------ | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------- |
| `prompt_templates.json`        | 内容无漂移；Windows 上因 CRLF 检出而误报                                       | `.gitattributes` 固定 `apps/zcode-cli-rust/** eol=lf`                                                         |
| `tool_schemas.json` 等工具资产 | 无 v3.14.3 漂移                                                                | —                                                                                                             |
| `agent_memory_templates.json`  | 生成结果随生成机平台变化：TS 追加 `path.sep`，macOS 生成 `/`、Windows 生成 `\` | 生成器改写为 `{memoryRoot}{sep}` 占位；Rust `memory_root_with_sep` 按 `MAIN_SEPARATOR` 且与 TS 一样不重复追加 |

修复后两项 `--check` 在 Windows 通过。

## 协议（`packages/shared/src/zcode-protocol*`）

| 变更                                                                   | Rust 影响                                               | 结论                                              |
| ---------------------------------------------------------------------- | ------------------------------------------------------- | ------------------------------------------------- |
| `display` 字段 `.catch(undefined)`                                     | 仅消费端宽松解析；Rust 为生产者                         | 无需跟进                                          |
| subscribe/hello 新增 `workflowRunDeltas`                               | Rust `subscriptions.rs` 按 `Value` 取字段，不拒绝未知键 | 兼容；Rust 不产 workflowRuns，能力位无意义        |
| delta 新增 `workflowRun.updated/removed`、wire-fault、workflow-runs-\* | 全部为工作流                                            | 随 WP5 工作流方案处理；当前 Rust 保持 unsupported |
| 命令载荷 `botDeliveryTarget`                                           | Rust `input_validation.rs` 不拒绝未知键，静默忽略       | 仅 CronCreate 使用，Rust 无 Cron；随 WP5          |

## CLI 行为（bootstrap/core/adapters，非工作流部分）

| 变更                                                                | 结论                                                                                                   |
| ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `background-tasks.ts`                                               | 格式化 + 工作流通知时长；Rust 无需跟进                                                                 |
| 内置技能包 `bundled-skills`（仅 `dynamic-workflows` 技能）          | 与工作流绑定；Rust 在支持工作流前不应暴露                                                              |
| sessions-index 高频节流、conversation publisher 的 workflowRun 编码 | 工作流高频事件专用；Rust 无该事件源                                                                    |
| subscribe resume 重放 `(base.seq, current]`                         | 非新增，Rust 仍只发新 snapshot；已列入 WP6                                                             |
| `workflow` 升为内置保留斜杠命令                                     | Rust 未实现斜杠命令目录与自定义命令/`/init` 展开（Host 走 method-not-found 降级）——**新增到 WP4 缺口** |
| `permission/service.ts`、`tool-allowlist.ts`                        | 随 WP3 权限模式一并对齐                                                                                |

## 新增缺口

- WP4：斜杠命令目录、自定义命令展开、`/init`，并遵守保留名（含 `workflow`）规则。
