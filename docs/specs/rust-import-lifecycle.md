# Rust TS 历史导入的事务与文件生命周期

## 规则与所有者

Store 的独占阻塞 worker 是导入状态的唯一写入者。原 TS 数据库只读；成功导入仍保留完整一致性备份，不能把它当作可删缓存。每个 source / workspace 的导入标记、会话、消息、行和 ACK 在一个 SQLite 事务提交。任何解析、附件、存储错误或提交前取消都回滚本次新增历史，不修改已有 Rust 会话。

不同 workspace 的进程通过 data-dir 范围的文件锁串行导入，等待锁期间可取消；不改变日常 Session actor 的 workspace owner 锁。幂等检查在导入锁内进行。已导入 workspace 再次启动不复制源库。

## 文件与崩溃边界

每次尝试创建专属 `ts-import-<UUID>` 目录，备份临时文件和内容寻址附件均属于该目录。备份完成、投影完成后，把备份移至同目录上级 `ts-backup-<UUID>.sqlite` 并同步目录，再提交包含该路径的导入标记。成功后保留附件目录和备份；失败回收本次目录和备份，不触碰其他导入的附件。

持有全局导入锁时，启动恢复检查上述新命名目录对应的备份路径是否被 rust_legacy_import 引用。无引用表示上次中断、尚未提交，才回收目录及对应备份。有引用表示提交成功，全部保留。仅处理严格 UUID 名称的真实目录，不跟随符号链接。数据库查询失败时不做清理。旧实现留下的备份、全局 imported-attachments 及已部分导入的数据不自动删除或重写。

## 取消与顺序

```mermaid
sequenceDiagram
  participant Main as Main / stdio
  participant Store as Store worker
  participant DB as Rust SQLite
  Main->>Store: import(source, workspace, cancellation)
  Store->>Store: acquire import lock; recover uncommitted attempts
  Store->>Store: incremental SQLite backup; project one session at a time
  Store->>DB: one transaction: session / rows / messages / ACK
  alt failure or cancellation before commit
    Store->>DB: rollback
    Store->>Store: remove owned attempt files
  else success
    Store->>Store: publish and sync backup
    Store->>DB: import marker + COMMIT
  end
  Store-->>Main: completion after rollback or commit
  Main-->>Main: ready only after successful import and live transport
```

增量备份每步最多 128 页，步骤间检查取消；源库 Busy/Locked 使用短等待并检查取消。会话和消息边界同样检查。SIGTERM、Ctrl-C、stdout 断管和启动导入期间 stdin EOF 都取消导入；Main 等待 worker 完成回滚/文件清理再退出，不报告 ready。运行期间 EOF 保留原来的 actor 排空语义，不取消已接受输入。

进程取消与 SQLite COMMIT 竞争时，以实际持久化结果为准：提交成功的导入必须保留文件；不能因为后来取消而删除已引用备份。单次 SQLite 操作及文件读写不可抢占，不宣称硬实时取消。

## 验收与范围

- 两个顺序会话，后者含缺失附件：失败后没有部分会话 / 消息 / ACK / 导入标记，也没有本次备份和附件；重复失败不增长磁盘；修复源后可成功导入并幂等重启。
- 已有原生会话及旧格式备份保持不变；源数据库字节不变。
- 中断留下的未提交目录与备份被回收；提交成功引用的目录保留。
- 真子进程在等待导入锁时 SIGTERM / EOF 能退出、不 ready、无新导入；锁释放后可重启成功。
- SQLite 分步备份取消有确定性的 Rust 测试，验证清理和原库可读。
- 保持 App 协议版本、desktop 连续 / mobile replay 语义不变。大历史按需加载、跨导入备份去重、成功备份保留策略及附件 GC 仍为后续范围。
- 投影必须按会话 / 消息 / part / 行线性推进：不得在每轮或每行重扫整个 `rows`。空 `turnId` 补全只在首轮之前产生的行上执行；actions 资格集合一次构建，再逐行判断（2026-11-02 修复，20k/50k 首次导入由 253s/1654s 降至 4.1s/9.7s）。
- 100k 轮 / 196 MiB 合成库实测：导入后 `rust-sessions.sqlite` 339 MiB + 保留的 `ts-backup-<uuid>.sqlite` 197 MiB，临时数据目录 535 MiB（约源库 2.6 倍）。在成功备份保留/去重策略确定前，仍按“完整一致性备份，不能当作可删缓存”处理。

## 附件 MIME 的降级规则（2026-09-24）

对应 TS `core/src/agent/file-part-hydration.ts::filePartToContentBlock`：

| MIME                              | 投影                                            |
| --------------------------------- | ----------------------------------------------- |
| `image/*`                         | image_url                                       |
| `video/*`                         | video_url（原先缺该分支，会落到 ensure! 失败）  |
| `text/*`（有 preview）            | 该 preview 文本                                 |
| `text/*`（data URL）              | 解码后的文本                                    |
| `application/pdf`                 | file（`file_data`）                             |
| 其他（audio、zip、octet-stream…） | 文本占位 `[Attached <mime>: <filename \| url>]` |

其他 MIME **不再让整份导入失败**：此前 `ensure!(mime == "application/pdf")` 会把「历史里有一个 zip/音频附件」
变成硬失败（`source remains unchanged`），真实用户因此完全无法迁移；TS 对同类输入只产出文本占位。
只读文件的解析改为惰性（`resolve()`），占位分支不再读取大附件字节。

验证：`escode-cli-rust-migration-scale.test.ts` 导入 120 会话（含 1.5 MiB 未支持 MIME 附件）约 0.9s 完成，
且 1 个会话的占位文本出现在 provider 请求中；导入幂等（`rust_legacy_import` 仅 1 行，二次启动行数不变）。

## 真实 runtime 产出的数据（2026-09-24）

`escode-cli-rust-migration-live.test.ts`：先用 **Node runtime** 在 fixture 环境里真实跑一轮
（`node escode.cjs app-server --stdio`，registry 模式指向本地 provider），写出会话、用户输入、
Write 工具调用与助手回复；再用 Rust runtime 导入同一目录，逐行核对 turnHeader / userInput /
toolCall(Write) / assistantText 都在。fixture 为此新增 `root`（调用方持有生命周期，不被清理）、
`command`/`args`（可指向 Node CLI，忽略 Rust 专属参数）与幂等的 workspace 创建。

## 已修复：并发写入下的导入 BUSY（2026-09-24）

`escode-cli-rust-migration.test.ts` 的「TS migration preserves workspace identity…」用例偶发失败，
报 `TS history import failed; source remains unchanged: database is locked: Error code 5`：

- 单独运行（`--test-name-pattern`）稳定通过；整文件运行时约 1/3–4/5 概率失败，与机器负载相关。
- 该用例会先后在同一 data dir 启动两个 runtime（本地 workspace 与 `ssh://` workspace），
  两者共享 `rust-sessions.sqlite` 并对同一份 TS 源库导入，属用例自身的并发场景；
  源库读的 busy timeout 已从 20ms 放宽到 5s（见 rust-state 注释），仍不足以覆盖极端竞争。
- 属**既有偶发**：在本轮改动之前的同一套件运行里也出现过同样报错，与 fixture 的 root/command/args 改动无关。
  **根因**：导入事务用默认 DEFERRED，先取读快照、写入时再升级；期间同 data dir 的另一个 runtime
  写入会让升级直接失败为 `SQLITE_BUSY_SNAPSHOT`（同样是 `database is locked`）——**这种失败不受
  busy_timeout 约束**，所以表现是「几乎立刻报错」而不是等满 5s。

**修复**：导入事务改为 `TransactionBehavior::Immediate`（一开始就取写锁，冲突时按 busy_timeout 排队）。
`crates/state/src/legacy_storage.rs` 就地注明原因。修复前整文件运行 5 次里 4 次失败，修复后 5/5 通过。
这类失败对真实用户同样可达：同 workspace 两个窗口/Host，或导入期间另一个会话在写。

## 真实数据演练发现：model_change 时间线字段（2026-09-25）

用户授权的真实数据只读演练（`~/.escode/cli/db` 备份副本导入临时目录）发现：TS 自 migration 0020 起把
model_change 的真实选择写在 `toModelSelection` / `fromModelSelection`，`toModel` 只保留给旧 Reader 的
兼容对象（`providerID`/`modelID`/`variant`）。Rust 导入读的是 `toModel.providerId`，得到 null，
产出的 `timelineMarker` 不满足任何 `modelChange` 变体，App schema 拒绝整页 rows。

规则（对齐 TS `decodeStoredPart`）：只读 `toModelSelection` / `fromModelSelection`（`providerId`、`modelId`、
`options.reasoningLevel`），忽略存储的 `toModel`/`fromModel`；`toModelSelection` 缺失或无效时不产出该标记
（TS 解码后没有 `toModel`，Node 投影同样跳过）；`fromModelSelection` 不完整时按 ∅→X 标记处理。
既有合成用例使用 0020 之前的形状，因此一直未暴露。

### 同次演练发现的其余差异（均已修复，2026-09-25）

以 Node runtime 打开同一数据的独立副本作为 oracle，逐会话比较行种类计数、附件与 todo：

| 问题                     | Node（oracle）                                                                                                                                              | Rust 原行为                                                              | 修复                                                                                    |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------------------------------- |
| conversation_rewind 分支 | 活跃分支 = `keptMessageIDs` + `branchCutAfterMessageID` 之后追加的消息（`@escode/contracts` `selectActiveConversationBranch`）；无 `targetMessageID` 不过滤 | 按旧式 `messageID`/`partID` 截断；`messageID` 指向首条消息时整段历史丢失 | `domain/rewind_branch.rs` 逐行移植，750 条 TS 语料校验（`--check`）                     |
| 空文本推理               | `reasoning` part 文本为空时不成行（部分供应商只在 metadata 保存加密推理）                                                                                   | 产生空 `reasoning` 行                                                    | 空文本的 text/reasoning part 不成行；模型上下文不受影响                                 |
| model_change 标记        | 无来源的 ∅→X 边界总是落；有来源时首轮之前不落（silentInitial），之后仅在模型身份改变时落                                                                    | 每个 part 都落                                                           | 按 Node 规则；首轮之前的标记归属随后的第一轮（否则 `productTurnId` 为空被 schema 拒绝） |
| 无思考深度的会话         | 省略 `modelSelection.options`                                                                                                                               | `reasoningLevel: ""`，App schema 拒绝整份快照                            | 为空时省略 options，`thoughtLevels` 取空表                                              |

回归：`escode-cli-rust-migration-shapes.test.ts` 用真实 TS store 写出上述形态（不依赖用户数据），Node 与 Rust 打开结果逐行一致。

### 2026-10-31 演练补齐：冷导入行态与命令资格

整行差分比计数更严格，真实库暴露了以下冷导入差异（均以 Node runtime 打开同一副本为 oracle）：

| 行/字段                         | Node（oracle）                                                                                                                                                                   | Rust 原行为                                                                       | 修复                                                                                                                     |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| turnHeader `activeMs`/`endedAt` | 本轮起止取首条用户消息到末条消息（含 reasoning/tool 结束时间）的跨度                                                                                                             | 与 `createdAt` 相同                                                               | `message_end_at` + `finalize_turn` 按消息跨度合成；controlOnly 轮不写 `activeMs`                                         |
| turnHeader `outputTokens`       | 每轮合成 usage=0 的 ModelComplete，因此总写 0                                                                                                                                    | 缺字段                                                                            | 冷导入 header 固定写 `outputTokens: 0`                                                                                   |
| turnHeader `historyRoundCount`  | 本轮写入历史的模型响应数（assistant 消息数）                                                                                                                                     | 缺字段                                                                            | agent 轮按 assistant 消息数写入；controlOnly 轮只在 >0 时写                                                              |
| 行 `actions`                    | `materializeCommandRowActions` 收口：每轮最后一条 assistantText 可 fork；仅全局最新 realUser 输入可 edit；仅全局最新 assistantText 且同轮有可解析 realUser 来源时可 retry        | 完全不写 actions                                                                  | `state/legacy_row_post.rs::imported_actions` 逐条移植；资格以持久 transcript message 身份（`messageIdByRowId`）为界      |
| 行 `createdAtSeq`               | 合成事件号：TurnStarted 产出的模型切换标记/turnHeader/userInput 共享一号，其余行各自独立事件                                                                                     | 所有行共用同一值                                                                  | `imported_sequences` 按事件分组重编号；投影按首次出现排名比较                                                            |
| assistant 行顺序                | 按 part 顺序创建 text/reasoning 与 tool 行（同一条 assistant 内可交错）                                                                                                          | 先落全部正文行，再把工具行追加到最后                                              | `legacy_message.rs` 遍历 part 时就地建行                                                                                 |
| assistantText `state`           | 冷恢复由合成的 ModelComplete 收口，正文行恒为 `complete`；中断只体现在 turnHeader/session phase 与工具行                                                                         | 未完成消息的正文行也标成中断态                                                    | 正文行固定 `complete`；`mark_interrupted` 只改 header 与 session phase                                                   |
| toolCall 行字段                 | `entityId`=callID；`input`/`inputText` 来自 part state（Node `JSON.stringify(input ?? {})`，保留持久化键序）；`startedAt`/`endedAt` 来自 `state.time`；cancelled 行没有 `output` | 缺 `input`/时间；`inputText` 被 serde_json 按字典序重排；cancelled 行伪造提示文本 | 从 part 原始 JSON 文本抽取 `state.input` 并只压缩字符串外空白；pending/running 行在轮次结束时补 `endedAt`，不写 `output` |
| compact 摘要                    | providerContextOnly 整条跳过：不落行、不计 `historyRoundCount`、不推进 `endedAt`                                                                                                 | 先累加 round 并推进 endedAt                                                       | `provider_context_only` 在轮次统计前返回，仅恢复 context 摘要与 offset                                                   |
| userInput `attachments[].ref`   | `data:` 内联附件没有稳定内容引用时用 `turn-attachment/<rowId>/<index>`；稳定路径保留原 URL                                                                                       | 直接写 data URL                                                                   | 按 Node `attachmentMetasOfMessage` 同规则生成                                                                            |

`actions` 与 `createdAtSeq` 都只锚定真实存在 transcript message 的行；`synthetic` 输入和 hidden 消息不会获得 edit/fork/retry 资格。

回归：`escode-cli-rust-cold-rows.test.ts` 对 8 个冻结种子（`migration.real`/`migration.compact`/`migration.workspaces`、
`migration-shapes.shapes`、`session-context-differential.legacy`、`history-request-differential.legacy`、
`import-lifecycle.sessions`、`migration-boundaries.membership`）逐会话比较整份投影行；Node 观察冻结为
`cold-rows.frozen-seeds.json`，正常跑只启动 Rust。membership 种子的 fork/subagent child/workflow 无消息历史，
只验证可读且行集合为空；archived 会话在 Node 侧不可 resume，按产品保留范围排除在行对拍之外。

### 演练工具与结果

`escode-cli-rust-real-data-rehearsal.test.ts`：默认跳过；`ESCODE_REHEARSAL_DB=<db.sqlite>` 时以 SQLite backup 取副本、复制附件目录，Node 与 Rust 各开一份，逐会话比较行种类计数、整行内容 SHA-256（归一化 id/时间戳/路径）、首个差异 JSON 路径（`rowDiffPath`）、附件数、todo 数，只输出计数与哈希；`DEBUG_REHEARSAL=1` 额外打印结构与枚举字段（不含会话文本/路径）。断言原库与副本哈希不变。

本机实测（Windows，GNU 目标，用户授权）：3 个会话（3 个 workspace，43 条消息、126 个 part，含 2 个 conversation_rewind、6 个附件、6 个 todo、0 条工作流数据）全部一致；原库哈希前后不变。样本小，只能证明上述形态；其他机器与更大的真实库仍需用同一工具复核。

2026-10-31 复跑（Linux，用户授权，`~/.escode/cli/db/db.sqlite`）：1 个 21 轮会话的整行内容哈希两侧一致；先前的 `actions`/`createdAtSeq` 差异已收敛。
