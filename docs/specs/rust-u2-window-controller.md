# Rust 阶段 2：`window-controller` 域（rust-unified-host.md 子 spec，优先序 5 收尾）

> 状态：已交付（8/8，本地 source；远程 source 见 §5-1 登记）· 2026-10-15
> TS 真相源：`packages/desktop/src/host/windowHostControllerProjection.ts`（纯投影，oracle）、
> `packages/desktop/src/host/windowHostControllerService.ts`（source 聚合/刷新/路由）、
> `packages/desktop/src/host/windowHostSessionsIndexObserver.ts`（sessions-index 被动观察）、
> 协议与类型 `packages/shared/src/escode-protocol-v4/controller.ts`。
> 关联：`rust-u2-tasks-index.md`（tasks-index 读写语义）、`rust-u2-agent-v4.md`（V4 订阅/帧纪律）。

## 1. 范围

`ServiceChannels.WindowController = "window-controller"` 的 8 个成员：

| 成员                                                                       | 职责                                                                                               |
| -------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `subscribeControllerV4` / `resyncControllerV4` / `unsubscribeControllerV4` | Host 侧投影的 V4 订阅（`controller/workspaces`、`controller/tasks-index` 两 topic）                |
| `onDynamicControllerFrame`                                                 | 上述订阅的帧事件（`ResponseType.EventFire`）                                                       |
| `listTaskList`                                                             | 跨 source 的任务列表查询（membership 过滤 + 全文搜索 + 分页）                                      |
| `mutateTask`                                                               | 列表写操作（`pin`/`archive`/`delete`/`delete-archived`/`mark-read`/`mark-unread`/`open`/`resume`） |
| `deleteArchivedTask` / `deleteArchivedTasks`                               | 归档删除（单个/批量，批量返回逐项目标结果）                                                        |

为什么做它：这是当前唯一「已被 Rust Host 服务、但整条 channel 缺失」的活链路。dev 开关（`ESCODE_RUST_HOST=1`）下
Web 远控的共享 Host attachment 已连 Rust Host（`packages/desktop/src/main/index.ts:849` →
`webRemoteControlSharedHostAttachments.ts:136-153`），而桌面/手机的时间线、置顶、归档、全局搜索、命令中心全部走本 channel
（`packages/ui/src/hooks/useGlobalTaskList.ts`，10 处调用点 / 8 个源文件，另有 3 个测试文件）；现在返回 `NotImplemented` → 列表恒空。

不做（本片登记为缺口）：

- **远程 source**：TS 的跨 source 路由依赖远程会话注册表（`windowRemoteConnectionRegistry`，属 remote 域，优先序 9）。
  本片只做本地 source；携带远程 identity 的 scope 不贡献行（见 §5-1）。
- 桌面窗口自身 host 的进程无关化（`packages/desktop/src/main/desktopHostProcess.ts:762`）：需 realtime/broadcast/cron
  控制协议，独立工作包。
- `escode-task` 的分组写面（`createTaskGroup` 等）与 `escode-task` 写面九法：各自切片。

## 2. 所有者与数据流

**状态所有者**：

- **tasks-index（SQLite）**：任务行事实（唯一写者不变式不变——controller 只读，写仍经既有写面）。
- **`TaskIndexSyncer`**：sessions-index / workspace-config 的 V4 摄入状态与**会话摘要基线**（`Core.summaries`）。
- **controller 投影（本片新增，Host 内唯一实例）**：`sources`/`rows`/topic 水位（`logEpoch`/`seq`）/订阅注册表。
  它不持久化，只由索引 + 会话摘要重建。

```
tasks-index（单写者）──────┐
                          ├─→ 投影 rows（meta + membership + liveStatus/activity）
sessions-index 摘要 ──────┘        ▲                    │
（syncer Core.summaries）          │ 变更信号            ├─→ topic 快照/增量帧（logEpoch/seq）
                                   │                     └─→ 每连接订阅 → EventFire
 信号①WorkspaceEvents.task_list_changed（索引写后，已有）
 信号②syncer 会话摘要变化（新增钩子）   ③controller mutation 完成（本片内联刷新）
```

**时序（一次列表查询）**：

```
Renderer listTaskList(query)
  → 解析 workspaceScopes（本地）→ 注册/复用 source
  → 刷新（single-flight + 世代守卫：并发信号合并为「脏标记」，在飞读取结束后追加一轮）
       读 memberships（active/pinned/archived 三读合并）+ overlays（syncer 摘要）
       → replaceSourceSnapshot：生成 rows，发布 task deltas（一个批次 seq+1）
  → 搜索分支：走索引 DB 全文搜索（含 snippets），逐条与投影行合并
    非搜索分支：直接取投影 rows（membership 过滤）
  → 排序（createdAt/updatedAt 降序，tiebreak taskId localeCompare）→ 截断 → {items,total,hasMore}
```

**与 TS 的逐条对齐点**（实现时照抄，不做「更好」的偏离）：

| 项              | 约定                                                                                                                                                                                                                           |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 刷新时机        | subscribe **不**刷新（快照可能为空）；`listTaskList`、mutation、workspace 事件、摘要变化各自触发                                                                                                                               |
| 合并键          | source key = `local\0<identity‖path>`；行键 = `local\0<identity‖path>\0<taskId>`                                                                                                                                               |
| membership 三读 | active→`{pinned:false,archived:false,active:true}`、pinned→`{pinned:true,archived:false,active:true}`、archived→`{pinned:false,archived:true,active:false}`（后读覆盖先读）                                                    |
| overlay 叠加    | 标题覆盖规则（`titleOverridden !== true \|\| titleSource === "custom"`）、`titleSource==="custom"` 时置 `titleOverridden`、`updatedAt` 覆盖、`activity` 存在时 `status = statusFromActivity(phase)`、`pendingInteraction` 覆盖 |
| `liveStatus`    | 有 overlay 用 overlay；否则 `defaultLiveStatus(meta)`：`completed→completed`、`error→error`、其余 `idle`                                                                                                                       |
| 帧水位          | 快照帧 `fromSeq:0`、`toSeq:当前 seq`；每批增量 `seq+=1`；`logEpoch` 每 topic 一个（Host 启动时生成）                                                                                                                           |
| 增量生成        | 快照替换：先 removals 再 upserted（deep-equal 才发）；**overlay 替换只发 upserted，不发 removal**                                                                                                                              |
| 排序            | `allRows()`：`updatedAt` 降序 → `taskKey` `localeCompare`；列表项：`sortBy` 取 `created`/`updated` 降序 → `taskId` `localeCompare`                                                                                             |
| 错误语义        | 未知订阅 resync → `未找到 Controller subscription，subscriptionId=…`；地址无 source → `没有与任务地址匹配的 source`；离线写 → `本地 source 当前不可用`                                                                         |
| 空批量删除      | `taskIds` 为空直接回 `{deletedTaskIds:[],skippedTaskIds:[],failedTaskIds:[]}`（不碰投影）                                                                                                                                      |

## 3. 接口与形状

- 帧：`{topic, subscriptionId, logEpoch, fromSeq, toSeq, sentAt, payload:{kind:"snapshot",snapshot}|{kind:"deltas",deltas}}`；
  `snapshot.protocolVersion=1`；任务快照 `tasks[]`，workspace 快照 `workspaces[]`（`{workspacePath, workspaceIdentity?, sourceAvailability, connectionState}`）。
- 行：`{address:{workspacePath,workspaceIdentity?,taskId}, meta, membership:{pinned,archived,active}, sourceAvailability, liveStatus, activity?, searchSnippets?}`。
  本片不下发 `remoteSessionId`（§5-1）。
- 订阅应答：`{ack:{subscriptionId, mode:"snapshot", logEpoch}}`；**首帧与应答的顺序**：TS 在方法内同步发首帧、应答随后
  ——Renderer 侧先挂事件监听再订阅且对两种顺序都兼容（`windowControllerTaskListRegistry`），Rust 沿用 V4 面的暂存纪律：
  订阅应先登记订阅号再发帧，首帧不得早于同连接的事件监听登记。
- 实参形状：`ProxyChannel.toService` 打包成数组（取第一个当对象）。

## 4. 验收

1. **TS oracle 差分（最高优先级）**：
   - 投影帧序列：脚本驱动真实 TS 投影（`windowHostControllerProjection.ts`，纯函数）与 Rust 投影执行同一操作脚本
     （register/replace/tamper/mutate/subscribe/resync），帧序列按 L4 有序全字段对拍；`subscriptionId`/`logEpoch`/`sentAt`
     为随机/时钟值，归一化后登记（§5-3）。
   - `listTaskList` / `mutateTask` / 归档删除：真实 TS 服务（`createWindowHostControllerRuntime` + 真实 tasks-index 适配器）
     与 Rust Host（真实 `@escode/client`）在同一夹具库上执行同一操作序列，`toEqual` 全等。
2. **端到端**：真 WebSocket + embedded 实例（`crates/host-rpc/tests/support/mod.rs`），覆盖订阅 → 首帧快照 → 索引写触发增量 →
   mutation 返回值 → resync/unsubscribe。
3. **负向验证**：① 改坏 membership 分类（如 pinned 与 archived 互斥判断）→ 列表差分失败；② 关掉逐帧 deep-equal
   （每次全量 upsert）或去掉 removals → 帧差分失败；③ 搜索分支不做投影叠加 → 搜索结果差分失败；
   ④ 列表排序方向翻转 → `rustHostWindowController.test.ts` 差分失败（真实 TS 服务侧）。

## 5. 登记差异

1. **远程 source 不支持**：TS 把 `remoteSessionId` 的 scope 路由到远程 attachment；Rust Host 无远程注册表，携带远程
   identity 的 scope 不贡献行、`remoteSessionId` 字段不下发。可见条件：接入过远程 workspace 的窗口（含手机远控窗口）。
   移除条件：remote 域（优先序 9）迁移时接远程 source。
2. **作用域粒度**：TS 的 controller 是窗口 Host 内、每 attachment 一个（各自事件发射器与订阅表）；Rust 按 RPC 连接作用域。
   可见条件：同一 Host 被多窗口/多 attachment 同时连接时的订阅隔离语义（列表内容一致）。
3. **随机值**：`subscriptionId`（TS `randomUUID`）与 `logEpoch` 由 Host 生成，`sentAt` 为时钟；三条都在差分中归一化。
4. **刷新触发点**：TS 的 workspace 事件来自 `escode-task`（本地）或远程 scoped 服务；Rust 由 `WorkspaceEvents` 总线
   （同源）+ syncer 摘要信号驱动。语义等价；若实测出现缺失触发点，在交付记录里补记。
5. **`subscriberScope` 不转发**：沿用 `rust-u2-agent-v4.md` §6 的既有登记（本片摘要订阅由 syncer 发起，`existing-only`）。

## 6. 风险与对策

| 风险                         | 对策                                                                                      |
| ---------------------------- | ----------------------------------------------------------------------------------------- |
| 摘要信号丢失（刷新期间到达） | 脏标记 + 串行重跑（在飞结束后再跑一轮），不用超时掩蔽                                     |
| 投影与索引双写者             | 投影只读索引；所有写经既有 `escode-task`/controller mutation 路径，写完由同一总线触发刷新 |
| 帧风口径不一致               | 复用 `collation.rs`（`localeCompare` 等价）；帧/行构造照抄 TS 投影                        |
| Renderer 对首帧顺序敏感      | 订阅登记先于发帧 + 端到端断言首帧不丢                                                     |

## 7. 交付记录（2026-10-15）

| 切片        | 内容                                                                                                                                                                                                                                                        | 证据                                                                                                                                                                                               |
| ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| WC-1        | 投影 + topic 面：`subscribeControllerV4`/`resyncControllerV4`/`unsubscribeControllerV4`/`onDynamicControllerFrame`（连接作用域 + 首帧前的有界早期缓冲）                                                                                                     | `host-rpc/src/window_controller/{mod,controller,frames,projection,rows,scope,overlays}.rs`；单测 `projection_tests.rs`（6 例）+ `overlays.rs` 内测试（3 例）；e2e `tests/rpc_window_controller.rs` |
| WC-2        | `listTaskList`（本地 source：membership 过滤 / 搜索分支走索引 DB + 投影叠加 / 排序分页）                                                                                                                                                                    | `window_controller/list.rs`；e2e 覆盖非搜索与搜索两条分支（真索引 + 真会话 + `renameTask` 后按标题搜索）                                                                                           |
| WC-3        | `mutateTask` + `deleteArchivedTask`/`deleteArchivedTasks`（复用 `escode-task` 写面：`domains_task_state::{apply_state_patch, apply_unread}`、`domains_task_delete::{delete_archived_one, delete_archived_batch}`）                                          | `window_controller/mutate.rs`；e2e `tests/rpc_window_controller_delete.rs`（空批次短路 / 未归档 skipped / 归档后 deleted）                                                                         |
| oracle 差分 | 帧序列：`scripts/generate-escode-host-controller-fixtures.mjs`（真实 TS 投影驱动脚本）→ `controller_frames.golden.json` → Rust 重放对拍（17 步脚本 / 8 帧 / 3 条错误文案）                                                                                  | `window_controller/projection_fixture_tests.rs`；`--check` 已进 `pnpm check:escode-host`                                                                                                           |
| oracle 差分 | 列表/写面：真实 TS `createWindowHostControllerRuntime`（真实 `TaskIndexRepo` + 真实适配器读取面）vs Rust Host（真实 `@escode/client`）：同一夹具库上 8 组查询 + pin/archive/归档删除 + 两个写方向（Rust 写→TS 重读、TS 写→Rust 重读）`toEqual`              | `packages/desktop/test/rustHostWindowController.test.ts`                                                                                                                                           |
| 契约        | `host-proto` `IMPLEMENTED` 登记 8/8；`contract.rs` 双向对账                                                                                                                                                                                                 | `cargo test -p escode-host-rpc --test contract`                                                                                                                                                    |
| 负向验证    | ① membership 过滤改坏（`active => true`）→ 单测失败（`["b","d","c","a"]` vs `["b","d","a"]`）；② 去掉 removals → 单测 + TS 帧差分失败；③ 搜索分支去掉投影叠加 → e2e 失败（`sourceAvailability: Null` vs `"online"`）；④ 排序方向翻转 → 真实 TS 服务差分失败 | 见本文件 §4.3；四处均已复现并回滚                                                                                                                                                                  |

补充说明：

- **syncer 复用**：overlay 源是 `TaskIndexSyncer` 的会话摘要基线（`Core.summaries`），新增读 API
  `summaries()` + 广播信号 `overlay_changes()`（容量 64，消费方合并式刷新，`Lagged` 不阻塞）。
  runtime 侧无新增方法——`controller/*` 两个 topic 完全是 Host 侧投影。
- **刷新引擎**：`controller.rs`——每 source 一把异步锁（single-flight）+ 脏标记续跑；信号路径只置脏并由
  后台任务刷新，列表/写路径显式等待刷新完成。
- **`escode-task` 未加 controller 路由覆盖**：TS 的窗口 Host 用 `createControllerRoutedTaskService` 包一层
  `escode-task` 写面以便复用地址解析；Rust 侧写面本就直连索引并广播同一条 `WorkspaceEvents` 总线，
  controller 作为订阅者刷新即可，不引入第二条写路径（可见行为一致）。
