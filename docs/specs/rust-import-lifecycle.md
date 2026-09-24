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

验证：`zcode-cli-rust-migration-scale.test.ts` 导入 120 会话（含 1.5 MiB 未支持 MIME 附件）约 0.9s 完成，
且 1 个会话的占位文本出现在 provider 请求中；导入幂等（`rust_legacy_import` 仅 1 行，二次启动行数不变）。

## 真实 runtime 产出的数据（2026-09-24）

`zcode-cli-rust-migration-live.test.ts`：先用 **Node runtime** 在 fixture 环境里真实跑一轮
（`node zcode.cjs app-server --stdio`，registry 模式指向本地 provider），写出会话、用户输入、
Write 工具调用与助手回复；再用 Rust runtime 导入同一目录，逐行核对 turnHeader / userInput /
toolCall(Write) / assistantText 都在。fixture 为此新增 `root`（调用方持有生命周期，不被清理）、
`command`/`args`（可指向 Node CLI，忽略 Rust 专属参数）与幂等的 workspace 创建。

## 已知偶发：并发启动时的导入 BUSY（2026-09-24）

`zcode-cli-rust-migration.test.ts` 的「TS migration preserves workspace identity…」用例偶发失败，
报 `TS history import failed; source remains unchanged: database is locked: Error code 5`：

- 单独运行（`--test-name-pattern`）稳定通过；整文件运行时约 1/3–4/5 概率失败，与机器负载相关。
- 该用例会先后在同一 data dir 启动两个 runtime（本地 workspace 与 `ssh://` workspace），
  两者共享 `rust-sessions.sqlite` 并对同一份 TS 源库导入，属用例自身的并发场景；
  源库读的 busy timeout 已从 20ms 放宽到 5s（见 rust-state 注释），仍不足以覆盖极端竞争。
- 属**既有偶发**：在本轮改动之前的同一套件运行里也出现过同样报错，与 fixture 的 root/command/args 改动无关。
- 未用「再加长超时」掩盖：需要的是跨进程导入串行化（例如按 data dir 的导入锁），
  与「大库/多 runtime 并发导入」一并列入数据迁移的后续工作。
