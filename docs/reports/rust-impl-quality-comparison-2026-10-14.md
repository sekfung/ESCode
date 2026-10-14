# Rust 实现质量对比报告（ESCode vs ZCode-rs）

> 日期：2026-10-14
> 方法：按功能域双端读实现代码逐项对比，只评"同源代码分化后的差异"；证据为 file:line。关联：`rust-unified-host.md`（迁移计划）、`rust-impl-quality` 的改进项可直接进各期工作包。
> 定位：本报告回答"同一个功能谁的实现更好、我该抄什么、该保留什么"，是修改与优化的输入清单。

## 1. 域结论汇总

| 域 | 总体 verdict | U（我们）最大的问题 | U 最大的优势（保持） |
| --- | --- | --- | --- |
| Bash 权限/解析 | Z 更好 | registry 用 `Value` DOM 下标遍历 + unwrap；语料只比 bool | guarded 维度、规则/guarded 语料更广 |
| Engine 核心循环 | Z 更好（差异最致命） | 无 `catch_panic`：run panic 后 `Finished` 永不发、收尾永久挂死；46 处游离 spawn；字符串方法分派 | commit-barrier 测试同源同强 |
| 存储层 | Z 更好 | 启动错误只有字符串；legacy 投影无 TS 逐字段对照 | **owner 锁 + exit 75**（Z 已删锁）、导入失败原子性 |
| 工具层 | 分裂 | 进程树缺 B2/B3/B7 修复与跨进程孤儿回收；Edit 无回归语料 | Read 去重桩、搜索 prelude、**Windows 代码页解码**、**跨进程文件锁**（Z 没有） |
| 模型/网络 | Z 更好 | 无 net 单入口（6 处 env 直读、3 处自建 client）；配置每秒真读+SHA256 | model-IO JSONL 轨迹、断流恢复文档 |
| 协议/投递 | Z 更好 | **subscriptions.rs 342 行零测试**；无输出水位；无 `v4/command` schema 校验 | EOF 停滞看门狗、`raw_params` 键序保留 |
| 扩展体系 | 分裂 | 插件 40 个文件混在 tools 无边界；hooks 全交 TS 且 Rust 零测试 | 子代理权限模式完整度、cron 语义+corpus |

**总判断**：多数差距是"同源代码的后续修复/重构我们没同步"（Z 在同一条代码线上继续演进的结果），而不是两边独立设计分歧——这意味着大部分可以直接移植（B2/B3/B7、TaskScope、ChunkWindow、FileStamp、delivery 状态机），处理成本远低于重新设计。反向也有：我们有一批 Z 没有的实现（文件锁、代码页解码、OAuth、cron、model-IO），不要在对齐时抄丢。

## 2. Bash 权限/解析

| 维度 | 更好 | 理由 |
| --- | --- | --- |
| 解析架构 | Z | 逐函数移植 unbash 4.0.1（lexer 9 文件 + 真 AST/redirect arena + hang/overflow 显式建模）；U 是 1/3 体量子集，复合结构整体放弃 |
| 判定表 | Z | 同表驱动但 Z 全类型化（`Kind` 枚举、`SafeFlagValue`、`deny_unknown_fields`、回调 fn 指针加载期 panic）；U 的 safeFlags kind 是字符串、未知 kind 静默 false |
| registry | Z | Z typed `HashMap<String,Node>` O(1) + version/hash 指纹；U 1.9MB `Value` DOM + 下标 + 线性 find + unwrap（`bash_rule_prefix.rs:73-194`） |
| 语料 | Z | Z 全量逐字段 JSON + 种子化 fuzz（mulberry32）+ 病态输入性能预算；U 5224 条只比 0/1，逐字段仅 555 条 safe 子集 |
| guarded | U | 只有 U 有 guarded 维度（`guarded.rs` + 314 行 matcher + 1353 条逐字段语料） |

改进清单（按性价比）：
- [x] registry 类型化（typed `HashMap<String,Node>` + version/hash 断言），消掉 DOM 与 unwrap 面 —— 2026-10-14 完成：`bash_registry.rs`（Node/OptionSpec 位置数组反序列化 + `child()`/`option()` 访问器 + version 加载期断言 + hash 测试重算；生成器输出 `{version,hash,commands}`）；`bash_rule_prefix.rs` 全部改 typed 访问（原 `node[1]/[2]/[3]` 下标清零）
- [ ] `bash_readonly_corpus` 全量逐字段化 + 补种子化 fuzz（拼接+变异、超时/surrogate 过滤）
- [x] policy schema 收紧：enum + `deny_unknown_fields`（`bash_policy.rs:31-47`）—— 2026-10-14 完成：`bash_policy_tables.rs` serde 形状（`deny_unknown_fields`；`SafeFlagKind` 实现 `Deserialize`，未知 kind 即加载期错误），DOM 下标解析与 `unwrap` 清零
- [ ] callback 双向对账：`--check` 断言生成器导出的名字集合 == Rust 实现集合（现在是 `_ => true` 静默）
- [ ] 补病态输入性能预算测试（heredoc 陈旧目标、深嵌套、hang 边界）

## 3. Engine 核心循环

| 维度 | 更好 | 理由 |
| --- | --- | --- |
| 消息面 | Z | typed `Method`/`ServerMsg`/`Call`；U 字符串方法名 + `match as_str` 大表 + `_ => query` 兜底（拼错方法名变成查询） |
| 所有权 | Z | 单一 actor；U 另有 `CoreRuntime` 把 `Engine` 塞进 `Arc<Mutex<Engine>>` 持锁 await（`runtime.rs:14-55`），第二入口破坏 actor 不变式 |
| 崩溃安全 | Z | **Z 在 engine 循环与 run 任务两处 `catch_panic` 把 panic 转成错误事件；U 全仓 0 处（实测）**——run panic 后 EOF/close 收尾永久挂死（`engine.rs:249-277`、`session_close.rs:30-39`）。Z 还把 `Panicked` 与 `StorageCommitFailure` 同等视为"此后不得提交"（`engine_shutdown.rs:15-17`） |
| waiter | Z | Z 单注册表 `Waiters::release(owner)` + 单测；U 三张 `retain` 表易漏（`session_close.rs:26-28`） |
| 瞬态选项 | Z | Z typed `Submission` + `strip_transient`；U 把 automationId/offPeakTaskId 写进持久化 intent 再从 history 反查 |
| 任务纪律 | Z | Z Spawner 唯一入口 + 运行期 `running()` + clippy deny + 边界脚本；U 全 app 46 处 `tokio::spawn` 全 detached、无 lint |
| 遥测 chunk | Z | Z ChunkWindow 250ms 合并（firstChunk 直发、非 chunk 事实先 flush 保序）；U 每 delta 一条 fact |
| commit barrier | 平手偏 Z | 机制同源；Z 多 panic/权限持久化失败用例，且 harness 挂在 TaskScope 上顺带暴露任务泄漏 |

改进清单（P0 优先）：
- [ ] **移植 catch_panic**：engine 循环 + run 任务包住，panic → 失败终态事件；收尾加宽限 + 残余任务 warn；补 `a_panicking_run_still_reaches_a_terminal_phase` 用例
- [ ] `CoreRuntime` 标注"仅嵌入/测试"或删除，消除第二所有权模型
- [ ] 引入 TaskScope/Spawner（或最小版：spawn 门面 + 残余清单 + `.config` clippy deny），16+ 处裸 spawn 收拢
- [ ] waiter 合并单注册表；瞬态选项改 `Submission` 内存 map；`Guide` 握手换 typed 载荷
- [ ] 遥测接 ChunkWindow（250ms、携带 chunkCount/lastChunkAgeMs、firstChunk 直发）
- [ ] 评估自定义 slash 命令展开的两次 actor await（admission/出队各一次），移出 actor 或书面接受

## 4. 存储层

| 维度 | 更好 | 理由 |
| --- | --- | --- |
| 错误与启动 | Z | Z `StartupError{code,sqlite_code,migration_id}` 可分类可重试；U 除 `WorkspaceOwned` 外全是字符串（`storage.rs:120-129`） |
| 列表/索引 | Z | Z 原生列 SQL 下推、零 JSON 解码；U `json_extract` 投影 + 缺 path 时打开 TS 库回填（`storage_listing.rs:89-130`） |
| 迁移对照 | Z | Z DDL `include_str!` + 22 个 checksum 逐一断言、fixture 逐表 byte-identical、真实库 5397 会话对照、交替写/强杀 interop；U dwf_*/rust_* DDL 无机器校验、投影无 TS 逐字段 oracle |
| 锁与多进程 | **U** | U owner 锁 + exit 75 给 Rust-Rust 明确互斥、可重试；Z 已删工作区锁（`rust-m11-node-storage.md:98,171`），靠 Host lease，终端 `-p --resume` 可结算桌面运行中会话——Z 自认风险 |
| 导入工程 | U | U 的导入生命周期（尝试标记、取消、128 页步进、WAL 折叠、COMMIT 不确定保留）明显更细；Z 无导入层（读兼容） |

改进清单：
- [ ] 启动错误结构化（lock_timeout/corrupt/checksum/io 分类）
- [ ] DDL 漂移哨兵：rust_*/dwf_* 加 checksum/版本；导入时比对"TS 已应用迁移 id 集合"
- [ ] 导入对照 corpus：TS fixture → 导入 → 与 TS 冷读逐字段比对（补 rewind/shared/attachments 分支）
- [ ] 列表路径去掉"每次打开 TS 库回填"，导入时批量写 body 或一次性内存 join
- [ ] rowsRange 走 SQL 尾页（`(workspace,session,ordinal)` 已具备，U 独有机会）

## 5. 工具层

| 维度 | 更好 | 理由 |
| --- | --- | --- |
| Edit 匹配 | Z | 算法同源；Z 有 137KB Node-oracle fixture（`edit-match.json`）、`.ipynb` 拒绝、CRLF 多数表决、相似文件名建议；U 只有 100 行手写单测 |
| Read | **U** | U 有重复读去重桩、写后视图登记、`FileState` 模块更整；编码维度两侧同为 UTF-8-only（Z 也没有 read.encoding，那是 Node 基线行为） |
| 搜索 | **U** | Grep/Glob 逐行等价；U 多移植了 embedded-search prelude（bfs/ugrep/rg 注入 + 语料） |
| 后台进程树 | Z | Z 有 B2（cmd `raw_arg`）、B3（快照失败退路 killpg）、B7（setsid）三修复 + 跨进程孤儿账本/sweep + Drop 杀整组；U 的 `process_tree.rs:201` snapshot 失败直接报错（一个信号都不发），全仓无 setsid |
| 输出保序 | **U** | U stdout/stderr 共用一个 OS 管道天然交错有序；Z 两条 pipe 再互斥拼合 |
| 原子写 | Z | Z rename 重试（EPERM/EBUSY/Win32 32/33）+ rename 后 fsync 父目录；U 均无 |
| 跨进程文件锁 | **U** | U `host/file_lock.rs`（目录+owner 协议、陈腐回收、FIFO）是唯一 Rust 实现；Z-Rust 没有 |
| 平台适配 | 分裂 | U 有 Windows 代码页解码（Z 是 `from_utf8_lossy`，中文 Windows 会乱码）与 credential_cipher；Z 有 kit 共享库与 RedactedString 分层 |

改进清单：
- [ ] 进程树三补丁：snapshot 失败 fallback killpg、Bash/hook 子进程 setsid、任务 abort 时对整个进程组 SIGKILL（参考 `proc/tree/posix.rs`、`group.rs`）
- [ ] 跨进程孤儿账本（pid+启动时间+owner 身份，下次启动 sweep）或至少退出钩子清扫
- [ ] Edit 语料化：TS oracle 生成 `edit-match.json` 形态 fixture；补 `.ipynb` 拒绝与 `detectLineEndings`
- [ ] `file_atomic.rs` 加 rename 重试 + 父目录 fsync；与 `file_lock.rs::write_private` 合并
- [ ] read.encoding：实现（读记录编码、写回原编码）或在 spec 显式登记差异

## 6. 模型/网络

| 维度 | 更好 | 理由 |
| --- | --- | --- |
| 流式解析 | 平手偏 Z | SSE 逐字节同源；Z 有 server 块门控 + native_search 错误码；U 有 `delta.reasoning` 兜底与 anthropic_body 拆文件 |
| 重试/续写 | Z | Z 抽 `provider_auth` + Attempt/Reporter 生命周期、idle 阶梯计入 recovery（U 恢复时 attempt 回落 1 重置阶梯）、取消时补发 cancelled 状态 |
| 配置热读 | Z | Z FileStamp/ChangeGate（len/mtime/inode/ctime + 60s 兜底）文件未变零读零哈希；U 每秒真读两份 ≤8MB+SHA256（`registry.rs:86-114`），且无 registry 测试 |
| 网络出口 | Z | Z `net` crate 单入口（Purpose 三 client、CA 替换、gateway 改写、device id、子进程 env）+ clippy disallowed-methods；U 6 处 `std::env::var*`、3 处自建 client、无 coding-plan 网关改写、api_key 是裸 String |
| 失败现场 | Z | Z `FailureDetail` + `describe` + 脱敏 + 状态事件带 providerKind/baseURL/头/异常类型；U `ModelStatus` 字段少 |
| 可诊断性 | **U** | U model-IO JSONL 轨迹（每次尝试落盘）+ 用量事实准入票据，Z 完全没有——保持 |

改进清单：
- [x] 收拢网络出口（新 net crate 或先搬 RuntimeEnv/Purpose client 三件套），删 model crate 6 处 env 直读，密钥改 RedactedString —— 2026-10-14 完成：`crates/net`（Purpose 三 client + 代理/CA 单一解析 + rustls provider 归位）+ `RedactedString`（core-api::secret，api key 与配置头脱敏）+ clippy `disallowed_methods=deny`；env 直读在阶段 1 已清零
- [ ] registry 加 FileStamp/ChangeGate + 五类稳定性测试（出现/改写/原子替换/删除/账号旁路）
- [ ] 补 Coding Plan 网关端点改写 + 显式 Host 摘除
- [ ] 拆 `provider.rs`（provider_auth/idle/stall/cancelled 收口）；`ModelStatus` 加厚（脱敏头、providerKind、异常类型、timings）
- [ ] Rust 进程内 mock SSE（40 片 3ms 形态可照抄），让重试/协议解析可 `cargo test` 确定性差分

## 7. 协议/投递

| 维度 | 更好 | 理由 |
| --- | --- | --- |
| 投递分层 | Z | Z delivery crate 独立状态机（每 (topic,connection) 唯一订阅、recovering 计数、pin 释放、topicOpen 回复后且首帧编码成功才登记）；U 三层混在 `subscriptions.rs`（342 行）且**零测试** |
| 帧编码 | Z | Z 按三种载体 `measure()` + 二分分片预算 + fail closed；U 固定 512KiB/900KiB 阈值，未计 relay base64 4/3 膨胀、无分片数上限 |
| 命令校验 | Z | Z 从 TS zod 生成 `v4-command.json` + 自研 json_schema 子集校验器 + 漂移测试；U 仅 serde 失败粗粒度拒绝 |
| 键序保留 | **U** | U `raw_params` 保原键序（协议层），与 schema-order 规格配套 |
| 背压 | Z | Z 自测 sink backlog 高低水位滞回、拥塞丢缓冲转快照；U 输出侧无字节水位，Host 不读即阻塞，依赖 Host 上报单点 |
| EOF 停滞 | **U** | U 的 1s 停滞看门狗取消运行时（`stdio_input.rs:126-151`）是实测修复，Z 没有 |
| MCP 结构 | Z（略） | Z 有进程账本/进程遥测/资源采样独立模块、插件声明归 plugins；U 的 `mcp_config.rs` 395 行混 5 类职责 |
| MCP 测试 | Z（投递）/U（规则） | U 有 MCP OAuth/官方 auth 逐字段语料（TS 侧另有 negotiation/result differential 测试）；Z 有 delivery harness + 行为测试覆盖 ACK 先于首帧等 |
| OAuth | **U** | U 全套 OAuth（flow/http/store/credentials/client + 后台 300s 授权事务 + 语料，约 1.9k 行）；Z 直接拒绝 oauth 配置（`mcp_connection.rs:64-68`） |

改进清单：
- [ ] 补投递层测试（ACK 先于首帧、窗口、拥塞丢缓冲快照恢复、重复订阅替换、关闭连接后在途回复不登记）——当前零覆盖是最大风险面
- [ ] 输出加字节级 backlog 测量 + 高低水位滞回，去掉"Host 不上报就无保护"
- [ ] `subscriptions.rs` encode 换载体感知预算（含 base64 膨胀）+ 分片上限
- [ ] `v4/command` 引入 schema 校验（可移植 json_schema 子集校验器 + TS 生成）
- [ ] MCP 拆 `mcp_config.rs` 职责；补进程账本/遥测
- [ ] 保持 OAuth 与输入 schema 键序（U 优势）

## 8. 扩展体系（插件/skills/hooks/子代理/cron）

| 维度 | 更好 | 理由 |
| --- | --- | --- |
| 插件拆分 | Z | Z 独立 plugins crate（50 模块）+ Ports 依赖倒置 + 模块级测试；U ~40 个 `plugin_*.rs` 与 MCP 等混在 25.4k 行 tools |
| 安装 | Z（略） | 逻辑同源；Z 类型化 `Failure` + `Activation` 事务对象 + 副作用外移；U 错误靠文案前缀匹配（`plugin_install.rs:288-311`）、install_locked 单函数 6 职责 |
| skills | 平手偏 Z | Z 扫描单点复用；U SKILL.md 扫描复制三处（`tool_skills.rs`/`plugin_describe_scan.rs`/`plugin_list_components.rs`）有漂移风险；U 多 bundled/visualize 增量 |
| hooks | Z | Z 全量 Rust（runner/trust 六态/digest/schema/admission，3.7k 行 + 单测）；U Rust 只做投影，执行/信任全交 TS，Rust 侧 5 个文件零 `#[cfg(test)]` |
| 子代理 | **U** | U 权限模式三分支（auto/plan/Explore-yolo+guarded 收紧）、mailbox 生命周期、TS 逐条对齐注释；核心调度同构 |
| cron/offpeak | **U** | U 语义在 CLI（守卫/文案/disallowlist/输出序）+ corpus 差分；Z CLI 侧零实现（其调度在 Host，不在该快照） |

改进清单：
- [ ] 抽 `crates/plugins` + `Ports` 注入（先迁 atomic_dir/git/zip/install 闭环）
- [ ] 插件错误类型化（enum + downcast 取 code）
- [x] SKILL.md 扫描合并为单一实现 —— 2026-10-14 完成：`crates/tools/src/plugin_skill_files.rs`（逐字对齐 TS `scanSkillFilesUnderRootSync(followSymbolicLinks:false)`：根自身+一级子目录、真实文件、不跟随符号链接、**readdir 顺序不排序**），三处调用（`skill_files_under` / `count_skill_files` / `plugin_describe_scan`）全部改走单点；含扫描与逃逸防线单测，插件 describe/control-plane 差分对拍通过
- [ ] hooks 最小 Rust 化：`HookEvent`/matcher 校验提为 domain 类型 + `hook_rows` 投影补单测
- [ ] 保持子代理与 cron 的现有实现与语料

## 9. 纠偏（前几轮结论的修正）

- Z 并非"跨进程锁在 base crate"：CLI 侧工作区锁**已删除**，只靠 SQLite 锁 + Host owner/lease（其 spec 明文）；跨进程文件锁目前只有我们有。
- Z-Rust 也没有 `read.encoding` 保留（非 UTF-8 两侧都直接报错）——那是 Node 基线的行为，Z 审计把它列为自身差异；我们要么实现要么登记。
- U 的 MCP differential 测试（negotiation/result）存在，在 `packages/services/tests/` TS 侧。
- Z 的 fixture `--check` 与 `test:rust-host` **不在 CI**（本地门禁）；我们的 27 个生成器 + 163 个差分用例已在 CI——别抄它的退化。

## 10. 总改进清单（按优先级）

**P0 正确性（本周）**
1. catch_panic 移植（修 run panic 挂死路径）+ 收尾宽限/残余 warn + 用例
2. 投递层 `subscriptions.rs` 测试兜底 + 输出 backlog 测量
3. process_tree snapshot 失败 fallback（不再"一个信号都不发"）
4. registry 7 处 unwrap 与 mcp_hub 锁 unwrap 清理（对照 spec §4.6 欠账）

**P1 结构（2-4 周）**
5. TaskScope/Spawner 收拢 spawn + clippy deny（await_holding/disallowed_methods）
6. net crate 收拢 + RedactedString + FileStamp 配置缓存
7. registry 类型化 + version/hash；policy enum + deny_unknown_fields
8. waiter 单注册表；`Submission` typed；`CoreRuntime` 清理
9. 插件域拆 crate + Ports；错误类型化；skill 扫描单点
10. `v4/command` schema 校验

**P2 语料与打磨（持续）**
11. bash 语料逐字段 + fuzz；edit-match fixture；导入对照 corpus；hooks 投影单测
12. ModelStatus 加厚；Rust 进程内 mock SSE；确定性约定（Clock/UUID/realpath/SHELL）写入 Host 测试规范
13. rowsRange 尾页 SQL；插件安装副作用外移；MCP 进程账本/遥测

**保留清单（对齐时不要抄丢）**
owner 锁 + exit 75、单管道输出保序、Windows 代码页解码、credential_cipher、跨进程 file_lock、OAuth 全套、MCP inputSchema 键序捕获、model-IO 轨迹、EOF 停滞看门狗、embedded-search prelude、Read 去重桩、子代理权限模式、cron/offpeak CLI 语义。
