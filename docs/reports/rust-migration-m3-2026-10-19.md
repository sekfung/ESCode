# Rust 迁移 M3 报告：hooks 域（2026-10-19）

## 1. 基线与范围

| 项         | 值                                                                                                      |
| ---------- | ------------------------------------------------------------------------------------------------------- |
| 分支       | `m3/hooks`（从 `dev` `b50cb014` 新建）                                                                  |
| 开工 HEAD  | `b50cb014`（`fix: 稳定 dev CI（workflow actor shell 继承与测试竞态）`）                                 |
| 本轮提交   | `a89f0203` + 后续测试/报告提交（见 §7）                                                                 |
| 平台       | Linux x86_64（7.0.0-34-generic），无图形会话                                                            |
| Rust       | rustc 1.95.0（2026-04-14），cargo 1.95.0，与 `rust-toolchain.toml` 一致                                 |
| Node/pnpm  | v24.14.0 / 10.33.2（`mise.toml` 期望 Node 24.18.0，本机有 engine 警告）                                 |
| freshness  | 开工时 `node scripts/check-workspace-freshness.mjs` exit 0（dev 与 origin/dev 同步，ahead 232）         |
| 开工工作区 | `git status --short` 有 6 个他人未提交改动；本轮未提交、未修改它们；期间另一工作流将其提交为 `b50cb014` |

**本轮域选择说明**：任务模板中的「本轮域」占位为空。按清单顺序选择 **hooks**（3 个成员、全部有 Renderer 调用点、
直接阻塞 H2.6/H3.4），并在本报告登记该选择依据。

### 并发工作流

- 开工后另一工作流把 `dev` 从 `2554c6af` 推进到 `b50cb014`（含开工时的 6 个未提交改动）。本轮从 `b50cb014` 建分支。
- 期间主工作树曾被另一工作流切换回 `dev` 一次（reflog `HEAD@{0}`）；本轮用 `git switch m3/hooks` 恢复，
  未触碰他人的分支/工作树（`/tmp/escode-head-check`、`/tmp/claude-1000/...` 等）。
- 同一仓库的其他工作树不属于本轮，未删除。

## 2. 本域成员

| 成员                      | Renderer 调用点                                                   | TS oracle                                                                                                    | 实现位置                                                                                                         | 真实客户端/端到端                                                                                      | 备注                                                                               |
| ------------------------- | ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------- |
| `loadHooks`               | `packages/ui/src/store/hooksStore.ts:114,199,228`                 | `hooksService.ts:214-268`；`workspace-hook-config.ts:130-167,207-297`；`workspace-hook-digest.ts:62-190`     | `host-services/src/hooks/service.rs`、`config.rs`、`schema.rs`、`digest.rs`、`trust.rs`、`legacy.rs`、`paths.rs` | `rustHostHooksParity.test.ts`（TS 差分 + 存量读入）；`rustHostRpc.test.ts`；`rpc_hooks.rs`             | 项目/用户/legacy 顺序、snapshot、digest、trust 态与 TS 全等（`discoveredAt` 除外） |
| `saveHooks`               | `packages/ui/src/store/hooksStore.ts:133`                         | `hooksService.ts:287-308`；`workspaceHookSettingsModel.ts:70-91,251-283`；`workspace-hook-mutation.ts:29-59` | `host-services/src/hooks/write.rs`、`ordered_json.rs`                                                            | `rustHostHooksParity.test.ts`（L5 字节一致）；`rustHostRpc.test.ts`；`rpc_hooks.rs`                    | 未知顶层键/键序保留，2 空格 + 尾换行，0600 临时文件 + fsync + rename               |
| `grantWorkspaceHookTrust` | `packages/ui/src/settings/useWorkspaceHookInlineTrust.ts:100-103` | `escodeAgentService.ts:4258-4273`；`bootstrap/src/escode-protocol/workspace-hook-trust.ts:24-68`             | `host-rpc/src/domains_hooks.rs`（转发 `workspace/hooks/trustGrant`）                                             | `rustHostHooksParity.test.ts`（真实 `@escode/client`，最小 workflow host 桩成功/拒绝）；`rpc_hooks.rs` | Host 不直接写 trust store；reasonCode 原样透传；缺参数在 Host 侧 `InvalidRequest`  |

`hooks` 从 0/3 Pending 翻为 **3/3 Implemented**；全仓 **46 channels / 539 members / 337 Implemented / 202 Pending**。

## 3. 验收证据

### 3.1 命令与退出码

| 命令                                                                                                               | 退出码 | 关键结果                                                                               |
| ------------------------------------------------------------------------------------------------------------------ | ------ | -------------------------------------------------------------------------------------- |
| `pnpm check:escode-host`                                                                                           | 0      | 边界、8 个生成器 `--check`、inventory 46/539、fmt、clippy `-D warnings`                |
| `pnpm test:escode-host`                                                                                            | 0      | 全部 cargo 测试 + interop 1 passed；新增 `rpc_hooks.rs` 2 例                           |
| `pnpm check:escode-cli-rust`                                                                                       | 0      | 边界、fmt、clippy                                                                      |
| `pnpm test:escode-cli-rust`                                                                                        | 0      | 411 项：399 pass / 0 fail / 12 skipped（约 32.5 分钟）；本轮域不涉及 CLI 代码          |
| `pnpm typecheck`                                                                                                   | 0      | 含 `packages/desktop/tsconfig.e2e.json`                                                |
| `pnpm lint`                                                                                                        | 0      | 0 error / 161 warning（较 M2 的 160 多 1 条，均为既存/他人改动；本轮文件无新增 error） |
| `pnpm fmt:check`                                                                                                   | 0      | 8697 文件格式正确                                                                      |
| `pnpm architecture:check --changed`                                                                                | 0      | violations 0 / new 0                                                                   |
| `node scripts/generate-escode-host-service-inventory.mjs --check`                                                  | 0      | 46 channels / 539 members                                                              |
| `cargo test -p escode-host-rpc --test contract`                                                                    | 0      | 3 例通过                                                                               |
| `cargo test -p escode-host-services hooks`                                                                         | 0      | 14 例通过                                                                              |
| `cargo test -p escode-host-rpc --test rpc_hooks`                                                                   | 0      | 2 例通过                                                                               |
| `pnpm exec vitest run packages/desktop/test/rustHostHooksParity.test.ts packages/desktop/test/rustHostRpc.test.ts` | 0      | 2 files / 2 tests passed                                                               |

### 3.2 对拍级别

- **Rust 单测（L4）**：`loads_project_user_and_legacy_sources_in_typescript_order` 用真实 TS
  `createHooksService()` 生成的冻结摘要断言：
  - `hookDeclarationDigest = 0724c613fe3d6c94d698d8b0eff02c0c6c08833122c9164f37234762ab63fc6a`
  - `bundleDigest = e95f1c11577a2ab816df613e80fe7dcb5142412c6af3785f8efc03f2e219814c`
- **TS oracle 差分（L4）**：`rustHostHooksParity.test.ts` 用同一份 TS 写出的配置，经真实
  `@escode/client` 调 Rust `loadHooks`，与真实 TS `createHooksService().loadHooks()` 做 `toEqual`，
  仅把 `workspaceHookSnapshot.discoveredAt` 归一为 `<time>`（已登记差异 #2）。
- **持久化（L5）**：同一 `Hook[]` 分别由 TS/Rust `saveHooks` 写回，user/project 两个文件逐字节相等。
- **存量读入**：TS 先 `saveHooks` 写出 `.escode/config.json`（含未知 `features` 键）与 trust store，
  Rust `loadHooks` 读入并与 TS 结果全等；trust store 命中时 `trustState = trusted_persistent`。
- **真实客户端**：`rustHostRpc.test.ts` 覆盖 `loadHooks`（空 workspace）与 `saveHooks → 落盘 → loadHooks`；
  parity 测试覆盖三个成员的成功/拒绝路径与相对路径错误。
- **链路语义**：hooks 没有推送/订阅/流；`rpc_hooks.rs` 同时经 `/ws/host`（desktop-continuous）与
  `/ws`（web-remote-replayable）调用 `loadHooks`，断言同一结果。

### 3.3 实现抽查

- `loadHooks` 的项目发现：`paths::project_config_directories` 向上找 `.git`；找到则根→叶，未找到只取起点
  （复刻 `workspace-hook-config.ts:335-346` 的 `return [start]`）。
- 摘要：`digest.rs::declaration_digest` 的数组顺序、`null` matcher、`["unset"]`/`["set", value]` 编码与
  `workspace-hook-digest.ts:203-248` 逐字一致；冻结摘要已证明。
- trust fail-closed：`trust.rs::parse` 对未知键、非法 digest、重复键、非法时间戳统一返回 `None`；
  读取路径区分 ENOENT（空集、不 corrupt）与其它 IO/JSON/schema 错误（空集 + corrupt）。
- 保存：`write.rs::writable_hook` 先展开 `custom` 再写已知字段；`enabled` 用
  `configuredState.configuredEnabled/declarationEnabled` 的 TS 规则；`to_events` 按首现事件与 matcher 分组。

## 4. 负向验证

在独立 worktree `/tmp/escode-m3-neg`（detached `a89f0203`）中进行；`CARGO_TARGET_DIR` 指向主工作区
`apps/escode-host/target` 复用依赖缓存。

| #   | 变异                                                                                  | 命令                                            | 结果                                                                                                                                    |
| --- | ------------------------------------------------------------------------------------- | ----------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | 从 `inventory_services.rs` 删除 `("hooks","loadHooks")`（Implemented → 实际 Pending） | `cargo test -p escode-host-rpc --test contract` | **EXIT 101**：`方法表里实现了但清单未标 Implemented: [("hooks","loadHooks")]`；`清单标 Pending 但方法表已实现: [("hooks","loadHooks")]` |
| 2   | 从 `domains_hooks.rs` 方法表删除 `("saveHooks", save)`（清单仍 Implemented）          | 同上                                            | **EXIT 101**：`清单标了 Implemented 但方法表里没有（渲染器永远拿不到）: [("hooks","saveHooks")]`                                        |
| 3   | 恢复两处后重跑                                                                        | 同上                                            | EXIT 0：3 passed / 0 failed                                                                                                             |

清理证明：

```text
$ git worktree remove --force /tmp/escode-m3-neg && git worktree prune && git worktree list
/home/sekfung/Projects/ESCode    a89f0203 [m3/hooks]
…（其余工作树属于其它工作流，未触碰）
$ git status --short | wc -l
0
```

## 5. 已登记差异

与 `docs/specs/rust-u2-hooks.md` §8 一致：

1. 相对 `workspacePath` 被拒绝（TS 按进程 cwd `resolve`）；原因：Host 不读进程 cwd。
2. `workspaceHookSnapshot.discoveredAt` 是 Host 当前时间；不参与 digest。
3. trust store 时间戳只校验 RFC3339 子集；zod `.datetime()` 范围更大。
4. 旧配置里 `hooks` 非对象时按空对象处理（JS 展开字符串会得到字符下标键）。
5. `grantWorkspaceHookTrust` 依赖运行时/Node workflow host；不可用时返回错误，不本地写库。
6. 顶层/`hooks` 对象键序保留；数组与值逐字一致。
7. legacy hooks 的 `args`/`async`/`shell` 按 TS 原样保留。
8. Renderer 传入的 `custom` 等 passthrough 对象在 RPC 解码后键序为字典序（`Wire::Object` 是
   `serde_json::Value`）；值/结构一致，只在多键 `custom` 的持久化字节序上可见。

## 6. spec 漂移修正

| 位置                                           | 原表述                          | 修正                                                                           |
| ---------------------------------------------- | ------------------------------- | ------------------------------------------------------------------------------ |
| `rust-migration-acceptance-2026-10-18.md` H2.6 | `hooks` 3/3 Pending             | 本报告与顶部「后续」链接记录 3/3 Implemented；验收报告正文不改判，由复核人更新 |
| `rust-u2-host-scope-boundary.md:171-175`       | hooks 未逐行复核                | 本轮逐项读码并交付，见 `rust-u2-hooks.md`                                      |
| `rust-unified-host-handoff-2026-10-15.md:8`    | 51 channel / 619 成员           | 当前 46/539；M2 报告已记录，本轮重新确认                                       |
| `rust-unified-host-handoff-2026-10-15.md:213`  | `escode-task.createTask` 未实现 | 已过时；M2 报告已记录                                                          |

## 7. 提交

| 提交       | 内容                                                                  |
| ---------- | --------------------------------------------------------------------- |
| `a89f0203` | hooks 服务域 + Host 通道 + 契约清单 + spec + 单测/差分/真实客户端测试 |
| 后续提交   | `rpc_hooks.rs` 双链路语义；本报告与验收报告后续链接                   |

## 8. 未完成项与说明

1. **`grantWorkspaceHookTrust` 的真实 Agent authority E2E**：本轮真实客户端用例使用最小 workflow host 桩
   验证 Host→runtime→host 的转发、workspace 引用与结果透传；Agent authority 的策略/重新发现/写库语义由既有
   TS 测试覆盖（`workspace-hook-trust-protocol.test.ts`、`workspace-hook-trust-cli.test.ts`）。
   在 Rust 侧直接驱动真实 `escode.cjs` 的 grant 未执行（UNVERIFIED）；原因是该路径需要 Main 合成的完整
   runtime env（插件宿主根、provider 配置等），本轮未构造。移除条件：测试夹具提供完整 runtime env 后补一例。
2. **`pnpm test:escode-cli-rust`**：已完整运行，411 项 399 pass / 0 fail / 12 skipped；本轮未改 CLI Rust 代码。
3. **Windows/macOS**：本轮未在 Windows/macOS 实机运行；代码使用 `Path`/`PathBuf` 与平台分隔符，
   trust 时间戳/JSON 解析与平台无关。CI 三平台门禁由 `rust-runtime-ci` 覆盖。
4. **Renderer 设置页交互 E2E**：本域没有新增 UI 文案或交互；未运行 Electron E2E（无图形会话）。
5. **`loadHooks` 项目配置读取错误**：与 TS 一样静默跳过（不返回 `errors`），未新增可观测字段。

## 9. 全仓状态

| 指标         | 本轮后     |
| ------------ | ---------- |
| channels     | 46         |
| members      | 539        |
| Implemented  | 337        |
| Pending      | 202        |
| 本域 Pending | `hooks`: 0 |
