# Changelog

## [3.15.2](https://github.com/sekfung/ESCode/compare/v3.15.1...v3.15.2) (2026-10-24)

### Features

* 闭合 Rust 窗口 Host parity 与安装包验收 ([28a67de](https://github.com/sekfung/ESCode/commit/28a67de1e60cb72e142231963461d74d8dcd601a))
  * 完成 rust-u2-window-host §5 的 1–10：远程 workspace 显式 per-window Node Host、HostEvent 消费、真实启动门、窗口 env、恢复与打包
  * 修复 Rust Host 端口投递时序：renderer snapshot 前不投递 state/ServicePort，并在业务根接管前重试
  * 修复 MessagePort 与 /ws 的 SocketProtocol 帧转换、远程 Node Host InitLocal/就绪门
  * 恢复被误删的 file/media-preview 服务注册
  * 修复 Rust 模式 prepared quit 的窗口关闭时序
  * 新增窗口生命周期 E2E、双 attachment 集成测试；release CI 增加三平台安装包 Rust Host 启动门禁

* 接入 REA 逆向分析插件并优化 i18n 文案校验 ([cfd2057](https://github.com/sekfung/ESCode/commit/cfd2057332fb0f3a4eb54733f1005e06ce1170b7))

* 接通 CUA 权限并统一窗口 Rust Host attachment ([50310dc](https://github.com/sekfung/ESCode/commit/50310dc9928774a7840dec962e1a759a68481d54))
  * cua-permission 经 MainBroker 转发给 Main；Node Host 与 Main 共用同一套权限服务编排
  * ESCODE_RUST_HOST 打开时窗口直接 attach App 级 Rust Host，新增 ESCODE_RUST_HOST_BINARY 覆盖
  * embedded runtime 支持 LegacyImport，Host 首启按 Implicit 导入 TS 存量历史
  * 逐条登记 58 个非缺口成员，inventory 修正为 43 channels / 495 members / 437 Implemented / 58 Pending
  * 修复 MCP 文档既有格式问题，恢复 fmt 门禁

* 默认启用 Rust Host 并完成四平台安装包验收 ([5fd09ba](https://github.com/sekfung/ESCode/commit/5fd09ba02d36747c53ecded3168b07355318c955))
  * isRustHostEnabled 默认 true，显式 0/false 回退 Node Host
  * dev 与 E2E 在缺失时自举 Rust Host debug 二进制，E2E 显式传递仓库二进制路径
  * 更新默认模式 E2E、SSH/生命周期用例与规格文档
  * 依据 release CI run 39920171658：win-x64/mac-arm64/mac-x64/linux-x64 installed 全绿

* 实现 plugins/listUiSurfaces 与 Host listSurfaces ([f5fef39](https://github.com/sekfung/ESCode/commit/f5fef394db267303811eba4bf700f43db3593529))

* 收口阶段2验收并接入阶段3 Host 切换演练 ([592b945](https://github.com/sekfung/ESCode/commit/592b945c065c00662ab92b592ed62d4559ce582c))
  * automation 增加真实客户端/TS 差分；providers P6 与阶段2清单状态更新
  * ci-install-desktop 支持 --host-switch-drill，release installed 四平台执行
  * parity 差分支持 --record/--golden 与按平台冻结；修复官方插件夹具环境
  * settings interop 增加冻结夹具单向读入；回退矩阵与 AGENTS.md 同步
  * 修复 session-persistence 未使用导入导致的 lint error

* 收口阶段3 golden freeze 与 Host 切换演练 ([951360a](https://github.com/sekfung/ESCode/commit/951360acb94736c2edd0c6a0c35b25bb598bd345))
  * 抽出 escode-cli-plugins（P1 #9）并注入 Ports
  * desktop Host 与 CLI 确定性 oracle 冻结为仓内 goldens/seed
  * 修复 yazl/buffer-crc32 打包闭包与 Node settings 未知键丢弃
  * linux-x64 当前源码包本地跑通 node→rust→node drill

* 支持 SVN 单文件历史与 Blame ([ae50776](https://github.com/sekfung/ESCode/commit/ae5077602b71623d057fd9094a121368ed41338f))
  * 新增 getFileLog/getBlame 服务、共享类型与 Rust Host 实现
  * 文件历史复用历史弹窗；新增改动行入口与 SvnBlameDialog
  * svn 1.14 blame XML 不含行文本，改用同一修订的 cat 补齐
  * 补充服务/UI/Rust 差分测试与 manual-review SVN-UI-05

* 支持 SVN 分支拓扑图谱 ([1a3efb8](https://github.com/sekfung/ESCode/commit/1a3efb822d8c5e03489293bef7ddcd5537731ac9))
  * 新增 getBranchGraph：copyfrom 复制边 + mergeinfo 合并边聚合
  * Rust Host 实现 getBranchGraph，并补充 TS/Rust 差分测试
  * 新增 SvnBranchGraphDialog、分层布局、图谱入口与中英文文案
  * manual-review 新增 SVN-UI-06，6/6 通过

* 支持 SVN 工作区（状态、历史、提交与分支切换） ([1f3e3f6](https://github.com/sekfung/ESCode/commit/1f3e3f6aa3897fd82c91356c30dd704701ef4b60))
  * 新增 workspace VCS 识别与 SVN 服务、RPC 通道、CLI context 注入
  * 新增 SVN 状态/历史/提交/分支切换界面与版本控制设置
  * 补充 SVN 单测、E2E 场景及 E2E 镜像的 subversion 依赖
  * 修复 tsup 残留 escodeDataSizeWorker 入口导致的构建失败
  * 清理 AGENTS 中已下线的手机远控与 relay 规则

* 支持 SVN 合并并补齐 Rust Host SVN 服务 ([52472e7](https://github.com/sekfung/ESCode/commit/52472e70fc347ac641605fb2ad92a1e2f8ad0e44))
  * 新增 svn merge 服务、共享类型、RPC 与 Rust Host 实现，支持修订范围与冲突
  * Rust Host 实现全部 svn channel 成员，并补充与 TS 服务的差分测试
  * 新增 SvnMergeDialog、合并入口、中英文文案与 UI 单测
  * 扩展 manual-review SVN E2E（新增合并场景，4/4 通过）
  * 更新 SVN 需求、架构与 UI 文档

* 支持 SVN 外部定义、锁定与暂存架 ([8f5a89f](https://github.com/sekfung/ESCode/commit/8f5a89f941d08883d9e9995ad164f63402165ed1))
  * 新增 getExternals/lock/unlock 与 diff/patch 可移植暂存架服务
  * Rust Host 实现对应成员，并补充 TS/Rust 差分测试
  * 改动行新增锁定状态/入口；新增外部定义与暂存架弹窗
  * manual-review 新增 SVN-UI-07，7/7 通过

* App-Provided Tools 与 Host 18 成员收口 ([b53281f](https://github.com/sekfung/ESCode/commit/b53281fb857237159813edfa2f640b14f173a44c))

* **bots:** 从 V4 队列差分推导排队群输入的丢弃提示 ([c15a22a](https://github.com/sekfung/ESCode/commit/c15a22aa18ba09e016226600accdba7999164af1))
  * botConversationStream 跟踪 queue.items：排队项消失且未成为输入（无同 sourceCommandId 的轮头/userInput、未进入提升或引导）时产出 turn_steer_status discarded；增量帧记 queue_deleted，重订阅快照记 session_resumed，首帧只建基线
  * botsService：Bot 自己发起的取消在调用前登记、失败撤销，流处理跳过，避免与“已取消该排队输入”重复
  * 旧 legacy 路径从未产出非 drained 的 turn_steer_status，该提示此前从未生效；spec 同步说明

* **bots:** Bots 回复流改为直接消费 V4 conversation 帧 ([89eee7c](https://github.com/sekfung/ESCode/commit/89eee7c48c93ac14b707db37fa207f73078a5c09))
  * 新增 escode-task.onDynamicConversationFrames：独立 background 持续订阅，seq gap 重订阅，dispose 释放；TS adapter（conversationFrameFeed）与 Rust Host 各实现，inventory 77/77
  * 新增 Bot 本地投影 botConversationStream：轮次/正文与思考后缀/工具/权限与问答/终态；首帧只建基线，终态排在帧末，traceId/inputId 取 sourceCommandId
  * botsService 优先走 V4 帧，旧远端 Host 回退 legacy 订阅；修复 Rust runtime 下 Bot 收不到回复流
  * 更正 onError 判断（TS 从不投递，Host 已对等）；新增 bots-v4-stream spec，更新 tasks-index / parity-remaining

* **circuit:** 接入立创EDA 原理图（.esch2）作为确定性输入源 ([11ab438](https://github.com/sekfung/ESCode/commit/11ab438b796c9365d1b652b881c22cc1084695f4))
  * domain/easyedaLog.ts：V3 日志解码——DOCHEAD 划分文档，(type,id) 按 ticket 收敛、同票比 client，
  * domain/easyedaSchematic.ts：SYMBOL 文档的 PART/PIN（NAME/NUMBER）与 SCH_PAGE 的
  * domain/easyedaDesign.ts：引脚按文档口径变换（绕原点逆时针旋转 → 镜像 → 平移）后与导线同一坐标系，
  * shared 新增 source kind easyeda_sch；sourceExtraction 按 .esch2 分派；面板入口与文件树加入 .esch2。
  * 新增手写 fixture 与三组测试（日志语义 / 原理图解析 / IR 装配）。

* **desktop:** Rust Host 进程崩溃检测与受限自动重启 ([57ea5b0](https://github.com/sekfung/ESCode/commit/57ea5b0db9e92a849f8ea760eb473deaa6cb670f))
  * rustHostManager 以“是否调用过 stop()”为唯一判据检测意外退出，状态机
  * 60 s 内最多 3 次自动重启，退避 500 ms / 2 s / 5 s；超出进入 failed，
  * 意外退出时已桥接的 attachment 全部 onLost（拆桥、关端口），远控管理器按 raw
  * 时钟可注入；新增重启、超限放弃、窗口外不计数、stop 不算意外退出四例

* **host-rpc:** 并发 RPC 连接层与事件面、Host 访问控制、escode-session 写面按 TS 收口 ([6a2df80](https://github.com/sekfung/ESCode/commit/6a2df800056323b7abf0c109117ac8320cb4080b))

* **host-rpc:** escode-agent V4 连接作用域——Rust Host 下首次跑通一轮真实对话 ([73b07ee](https://github.com/sekfung/ESCode/commit/73b07ee97139637a26fa2621df3c765986085cbf))
  * agent_v4：每条 RPC 连接一个作用域（TS createESCodeAgentConnectionScope 的移植）——hello/
  * 运行时错误按 TS 形状回 ESCodeProtocolClientError（透传 JSON-RPC code/data）；existing-only
  * 契约清单全量化：scripts/generate-escode-host-service-inventory.mjs 用 TS 编译器 API 解析
  * channel.rs 拆出注册表；连接级工厂的后台任务改走 with_open 钩子（探测方法表无副作用）。

* **host:** 服务契约清单与源码双向对账（方法表即分派） ([54a9ee1](https://github.com/sekfung/ESCode/commit/54a9ee14c4f702d59b940bd5e7aec8e74d87a893))
  * `host-rpc`: 各 channel 改为 `Channel{name, methods: Vec<(&str, MethodHandler)>}`，
  * `host-proto`: METHODS 清单拆分到 `inventory.rs`（setting/system/file/escode-session
  * `host-rpc/tests/contract.rs`: 双向机器对账 3 例——Implemented 必须在真实方法表里、
  * `domains_file_channel.rs`: file channel 方法表独立成文件（行数边界）。

* **host:** 旧 plugins 通道（整套退役）迁到 Rust Host ([d01d592](https://github.com/sekfung/ESCode/commit/d01d59227f103ab7a32226e49d1cc3efb4d3233e))

* **host:** 任务目标注册表 + getTaskModelSelection（S2b 前置件） ([fa67ee8](https://github.com/sekfung/ESCode/commit/fa67ee83175dd72f5588fcaf4d0745b583a44151))
  * `task_targets.rs`：TS `taskTargets` 同构的进程内注册表。两条写入路径语义不同、不能合并：
  * **补齐已交付读路径的语义缺口**：`getTaskMeta`、四个列表分区、`listTaskList` 在 TS 里都会
  * `getTaskModelSelection`：注册表解析目标 → 运行时 `session/read` → 回 `settings.model.current`
  * 结构：`domains_task_support.rs` 收纳共用件（懒索引句柄/查询上下文/运行时句柄/参数取值），

* **host:** 任务索引 syncer——V4 topic 帧摄入（S3b）+ 评审修正 ([5319b7f](https://github.com/sekfung/ESCode/commit/5319b7fe07ff2f139b26afa5d18475b95f6ba607))
  * agent_v4::assembler：V4 物理帧装配器（TopicWireFrameAssembler 同构：分片重组、crc32/base64/
  * task_index_syncer：按 workspace 建 sessions-index / workspace-config 被动订阅（existing-only），
  * tasks_index::apply_agent_patch（TS applyAgentPatch 同构）；resumeSession(broadcastSnapshot:false)
  * model 同步索引 model 列（关闭 §6.5-5 登记差异）。
  * R1 host-runtime：available 在实例登记进槽位前发布，syncer 的 existing-only 订阅可能扑空并
  * R2 syncer：逐条 spawn 的索引写可乱序落库（索引停在旧标题）。改为每 workspace 串行写队列。

* **host:** 实例请求/应答与 escode-session 域起点（listSessions 转发 runtime） ([01b35bf](https://github.com/sekfung/ESCode/commit/01b35bf45c3dc460451e7ec58b45b9086929b729))
  * host-runtime：实例读端收敛为一次读循环——按 id 关联 call() 的等待方、其余帧扇出给
  * host-rpc/domains：escode-session channel（TS 该域即 Agent 协议转发）——listSessions 按 params
  * 测试：Rust 端到端（空列表、实例与能力清单、方法缺口）+ 真实客户端

* **host:** 实现 plugin-ui-bridge.prepareSandbox ([d42c62e](https://github.com/sekfung/ESCode/commit/d42c62e98adfa0c31a0e264bea2999b5ae0866f5))

* **host:** 注册 plugin-ui-bridge 资源与页面调用成员 ([f61b059](https://github.com/sekfung/ESCode/commit/f61b0596959e80108d97cd22e29c95a2979384f7))

* **host:** broadcast 域（本地模式）6 个成员迁到 Rust Host ([1f885ca](https://github.com/sekfung/ESCode/commit/1f885ca45c76e59d32df8f6a2fb205105f67b598))

* **host:** client-config.getOffPeakClientConfig ([e6bad98](https://github.com/sekfung/ESCode/commit/e6bad98d07b49f2518631974d9978935514085cd))
  * 复用 client/configs 原始信封缓存与 Off-Peak 隐藏 Provider 模型视图
  * ESCODE_OFFPEAK_MOCK/NO_PLAN 经 runtimeEnv 生效
  * 2 个纯函数单测；client-config 3/3 收口

* **host:** commands 写面四法迁到 Rust Host（5/6） ([c747f8e](https://github.com/sekfung/ESCode/commit/c747f8e11aaecb9d85024a0d3744b119b1c0336f))
  * 新建即启用：写盘后把该文件的 enable 覆盖删掉（启用态不落盘）；
  * 改名迁覆盖：编辑命令换了文件名时旧覆盖要迁到新路径，否则用户刚禁用的命令
  * 删除/禁用都把覆盖表收敛回最简形态（启用即删键，command 空了连键删）。

* **host:** commands.getPrimaryUserCommandsDirectory 迁到 Rust Host（1/6） ([387d3a2](https://github.com/sekfung/ESCode/commit/387d3a2f7e16ac37122e3f4229898e06dd9e11e2))

* **host:** commands.list 迁到 Rust Host（6/6 收口） ([78eeb47](https://github.com/sekfung/ESCode/commit/78eeb47982f126f83f584e314148bdc55219ea53))
  * 每个 agentSource 内先 workspace 再 user（按目录交错读会让 user .escode 抢在
  * .escode 是强优先级来源：同 scope 下读到有效命令后 .agents 不再参与；
  * 重名去重保留先出现的那条（因此 workspace 胜 user）。

* **host:** credential 域 3 个成员迁到 Rust Host ([e39db56](https://github.com/sekfung/ESCode/commit/e39db56b31c68d45bdca04abedce8ca0b6f079b0))

* **host:** escode-session 读面（listSessions/readSession/readSessionMessages 转发 runtime） ([afb3d96](https://github.com/sekfung/ESCode/commit/afb3d96e682942e0d32d26d9a951c97e1cafd89f))
  * domains：抽出统一转发路径 runtime_target（服务级参数 → workspace 路由键剥离 + CLI 形状
  * 新增 readSession（session/read）与 readSessionMessages（session/messages）转发；
  * 测试拆分：host-rpc/tests/{support,rpc_surface,rpc_session}.rs（含 >400 行拆分）；

* **host:** escode-session 写面转发（closeSession/setModel/setThoughtLevel/setMode） ([ecbddeb](https://github.com/sekfung/ESCode/commit/ecbddebd4cc1936aa411fcfdc0af51fc70889196))
  * forward.rs：把服务域共用的运行时转发路径独立成模块（runtime_target + runtime_call +
  * escode-session 新增四个写方法的转发（legacy session/* 由运行时翻译为 V4 命令走同一写路径）：
  * 未迁移方法显式缺口并在代码注释里写明原因（createSession/resumeSession 需 Host 侧特性门参数整形、
  * 测试：Rust 端到端（条件关闭 true/false、setModel 运行时错误透传、createSession 缺口）+

* **host:** escode-task 侧栏状态写与 workspace 事件总线（S3a） ([65c073f](https://github.com/sekfung/ESCode/commit/65c073f66c0a1149234a4b288f38509ee02c2ee6))
  * tasks-index 状态写（TS TaskIndexRepo.updateTaskState / clearTaskUnreadIfMatches /
  * escode-task：setTaskPinned / setTaskUnread / archiveTask / unarchiveTask / renameTask（含尽力
  * workspace 事件总线 + escode-task.onDynamicWorkspaceEvent（只登记监听，不启动 runtime）；
  * host_command.rs：Host 侧 V4 命令信封（uuid v7、进程稳定 clientId）、ACK 收口、CAS stale 重试。

* **host:** escode-task 查询面（listTaskList 全文搜索 + 分组结构读） ([c27edd8](https://github.com/sekfung/ESCode/commit/c27edd8cd8c3e7513f33dc3607649009936b8067))
  * `listTaskList`（TS `queryTaskList` 同构）：多 workspace 聚合（key 去重排序）、kind 四分区
  * 搜索摘要 `buildSearchSnippets` 逐条对齐：窗口半径 20/72、上限 4 条、近重叠合并、
  * `listGroupedTaskViewStructure`：group 按 bootstrap workspace 可见性过滤、成员全量返回
  * 存储层行投影补 `searchable_text`（仅摘要窗口用，TS 同列）。

* **host:** escode-task 常量/回收类 4 个方法迁到 Rust Host ([dae96ae](https://github.com/sekfung/ESCode/commit/dae96aee484ed31a94e508fc80d2bb275be60593))

* **host:** escode-task 第一批行为补齐（bot 队列字段、clearGoal、旧话题记录、Claude 导入） ([f446190](https://github.com/sekfung/ESCode/commit/f44619043a99f8d3dcce1f56c69bb3cf10d3e150))
  * runtime 排队项保留 botGroupSource/conversationQuotes 与私有 _botDeliveryTarget，提升与 guide 消费时恢复；userInput 行带 botGroupSource
  * V4 新增 CAS 命令 clearGoal（TS/Rust runtime 同语义）；Host goalSession 按 TS 文案生成 response，clear 不再 unsupported
  * TS 旧 bot_topic_context 不再注入模型历史，保存为 legacyTopicRecords 并在 session/messages 原位投影，readBotTopicSummaries 可读
  * runtime 承接 importedHistory.source=claudeCode，导入即建可续聊会话；删除 Host legacy 快照兜底
  * 修复 escode-cli-rust-fixture 默认 SSE 应答缺 event/end import
  * 同步更新 rust-busy-input / rust-goal / rust-app-p0 / rust-legacy-session-methods / rust-u2-tasks-index

* **host:** escode-task 分组视图写面迁到 Rust Host（5 个方法，TS oracle 差分） ([ccb915a](https://github.com/sekfung/ESCode/commit/ccb915a8e2ca88b640486796faf0e5817335149e))
  * tasks_index/write_groups*：新建组插到顶层排序最顶（单事务）、删组与其排序同事务、apply 先校验再单事务提交
  * host-rpc/domains_task_groups：参数解包 + 事件（rename/recolor/delete/apply 对每个 scope 广播 task_meta_changed，create 不广播）。
  * 登记差异（rust-u2-tasks-index.md §6.8）：apply 回包 nodes 恒空且不复制回包路径的隐式写回（TS 回包走旧 queryGroupedTaskView 会补顶层排序；
  * 验收：write_groups_tests 6 例 + rustHostTaskGroups.test.ts（真实 TaskIndexRepo 与 Rust Host 同脚本逐步比较分组结构与全部错误文案，并断言事件）；

* **host:** escode-task 快照读面（S2b：getTaskSnapshot* / getTaskConfigOptions） ([03cda4b](https://github.com/sekfung/ESCode/commit/03cda4bc097d07e43a8f273c7c817bc28e48434b))
  * 新增 crates/host-services/src/task_projection/：policy/visible/status（可见性与任务状态）、
  * domains_task_snapshot.rs 接 getTaskSnapshot / WithEtag / Body / Ref / ToolCallsSlice /
  * 期望资产生成器 scripts/generate-escode-host-task-projection-fixtures.mjs：用**真实 TS 适配器**
  * 验证：host-services 75 例（含 oracle 差分 7 例）、rpc_task_snapshot.rs 3 例（真 WebSocket +
  * 登记差异（子 spec §6.4.4）：灰度工具面 flag、legacy 分支、live tool projection、ETag 键序

* **host:** escode-task 列表分区读（listTasks/pinned/archived/deleted） ([0e7474b](https://github.com/sekfung/ESCode/commit/0e7474b393e199c31402c3fe3fd617aae0351080))
  * 存储层：`list_task_metas(TaskFilters)` / `list_deleted_task_ids`（`None` = 不过滤，
  * channel：`listTasks` / `listPinnedTaskIds` / `listPinnedTasks` / `listDeletedTaskIds` /
  * 夹具：清单 `escode-task` 改为 6 条 Implemented + 11 条 Pending（`listTaskList` 仍需
  * `domains_file`：新增泛型阻塞桥 `spawn_blocking_io` + `FromBlockingFailure`

* **host:** escode-task 命令队列迁到 Rust Host（S8） ([9b6ae7c](https://github.com/sekfung/ESCode/commit/9b6ae7c77b82d7c8a93f5997d7c7df738c62d9f5))

* **host:** escode-task 批量归档/原生日志/用量迁到 Rust Host（S4） ([7dff1fd](https://github.com/sekfung/ESCode/commit/7dff1fd9d7feebd3b720b86fd534100eeb8875b5))

* **host:** escode-task 任务控制与 legacy 桩迁到 Rust Host（S7） ([4c93685](https://github.com/sekfung/ESCode/commit/4c93685ab32e9b9bc070a5bb4b0093b98be74c2b))

* **host:** escode-task 生命周期迁到 Rust Host（createTask/resumeTask/closeTask） ([ce6a639](https://github.com/sekfung/ESCode/commit/ce6a639cef8a794568768c7def45ddcf7ced3c44))

* **host:** escode-task 剩余 16 个成员迁到 Rust Host（S9–S12） ([6687cff](https://github.com/sekfung/ESCode/commit/6687cffdb4fa2c1a15d7a9063ac8d070e2de18bb))

* **host:** escode-task 输入/停止/交互迁到 Rust Host（S6，5/6） ([78a8c09](https://github.com/sekfung/ESCode/commit/78a8c093cb3873e6333e2c16f30e445a89ec7758))

* **host:** escode-task channel 接 getTaskMeta（S1 通道接线） ([3cc871d](https://github.com/sekfung/ESCode/commit/3cc871dec71aba69555047007b23b331a1fa9f72))
  * `host-rpc/src/domains_task.rs`：索引读方法 `getTaskMeta`（懒打开 tasks-index、失败不缓存
  * 契约清单登记 `escode-task`：1 条 `Implemented`（getTaskMeta）+ 12 条 `Pending`
  * Rust：90 passed / 0 failed（含 `tests/rpc_task.rs` 2 例：真实 WS 端到端读数、缺失回无值、
  * **真实 TS Repo 差分**：`prepareTasksIndexStorage` 建库 + `TaskIndexRepo.seedTaskMetaIfMissing`
  * 门禁：`pnpm check:escode-host` 全绿（边界 + 生成器 --check + cargo fmt + clippy -D warnings）。

* **host:** escode-task.getModelTrajectory 迁到 Rust Host ([8800e2b](https://github.com/sekfung/ESCode/commit/8800e2ba6abb61d558b16194d75df43af3ac756e))

* **host:** escode-task.restartWorkspaceProcess 迁到 Rust Host ([12c62a6](https://github.com/sekfung/ESCode/commit/12c62a60f49888420a2a33b2a1795ef47a114b12))

* **host:** file 域 ensureConversationWorkspace（dataBaseDir 由 Main 提交） ([c074ea1](https://github.com/sekfung/ESCode/commit/c074ea161f2cf7c68850bf322b2294e7fd92f1a2))
  * 语义：`<dataBaseDir>/.escode/workspace/default`（TS `getConversationWorkspaceDir()`），
  * 路径来源：控制消息 `rpcListen.dataBaseDir`（TS `getDataBaseDir()`：设置项 dataBaseDir >
  * Main 接线：`rustHostBridge`/`rustHostManager` 增加 dataBaseDir 透传，`index.ts` 传
  * 顺带收口：`control.rs` 的裸 `std::env` 改为 `core_api::env`（单一 env 出口）。

* **host:** file 域补 readTextFile（文本切片 + 二进制启发式） ([778d50b](https://github.com/sekfung/ESCode/commit/778d50b90c076d0a0462f711a4bd8efb223993f4))
  * domains_file.readTextFile：stat 后非文件报 Path is not a file；offset=max(0,trunc(offset ?? 0))；
  * readFileRange/readTextFile 共用 read_at（EOF 短读）
  * 测试：21 单测（含偏移/截断语义、二进制阈值 0.3 边界、常见空白豁免）+ 真实客户端

* **host:** file 域补工作区创建两法（default/scratch） ([189ce1f](https://github.com/sekfung/ESCode/commit/189ce1f2620e2cddb3c59bef86a8ca75add7a44b))
  * domains_file_workspace.rs（从 domains_file.rs 拆出，守 400 行上限）：
  * <home>/ESCodeProject/<name>
  * file_channel 接收 home（与 settings/system 同源）
  * 测试：24 单测（默认/命名工作区路径与幂等、名称校验文案、EEXIST 占位）+ 真实客户端

* **host:** file 域补预览两法（readMediaPreview/readBinaryPreview + MIME 判定） ([496f1c8](https://github.com/sekfung/ESCode/commit/496f1c89fc6964278947eac9c32d34f5eede7d85))
  * domains_file_preview.rs（从 domains_file.rs 拆出，守 400 行上限）：
  * 测试：23 单测（base64 往返、扩展名大小写、视频/兜底类型、超限与非文件错误、extname 边界）
  * 真实客户端（readMediaPreview/readBinaryPreview 往返 base64 与类型）

* **host:** file 域工作区索引与搜索迁移到 Rust Host（file 17/17，TS oracle 差分） ([4c543fa](https://github.com/sekfung/ESCode/commit/4c543fa05e74df63f872544559947a1a4f82e08b))
  * host-services/file_index：FileIndexService（Host 内唯一，缓存 60s TTL / 4 项 LRU / .escodeignore 指纹签名，
  * 规则匹配用 `ignore` crate 并开启大小写不敏感（npm ignore 默认如此）；95 规则 × 69 路径语料上仅剩 7 条极端写法不同，登记于 spec §6.1。
  * host-rpc：并入 `file` channel 的 6 个方法；host-proto 登记；rustHostRpc.test 里“搜索是缺口”的旧断言改用 system.probeIntranet。
  * 验收：rustHostFileIndex.test.ts 32 套规则 × 夹具、30 个 query × 8 种 limit、区间边界、规则读/转换/写全等

* **host:** file 域精确子集（stat/checkFilesExist/resolvePath/readFileRange，优先序 4） ([e2761ab](https://github.com/sekfung/ESCode/commit/e2761ab8e6b0110fe7f51ff094081673b1f749a1))
  * 新增 crates/host-rpc/src/domains_file.rs（测试拆到 domains_file_tests.rs）：
  * readdir 不实现并登记原因：TS 第二排序键用 localeCompare（ICU），实测与码点序明显不同；
  * 测试：19 单测（形状/文案/顺序/批上限/边界/缓存 TTL 与 LRU）+ 真实客户端

* **host:** file-watcher 域迁移到 Rust Host（4/4 成员，TS oracle 差分） ([19e2125](https://github.com/sekfung/ESCode/commit/19e2125bd3f34ad19952c5619e09d612703ca5f8))
  * host-services/file_watcher：FileWatcherService（Host 内唯一）；notify 句柄 + 防抖 actor
  * host-rpc：`file-watcher` channel；host-proto 登记 4 个成员；依赖 notify 8。
  * 验收：rustHostFileWatcher.test.ts 同一脚本驱动 node:fs watch 与 Rust，事件序列逐场景全等；
  * macOS/Windows 上 Node 与 notify 的事件粒度未验证（spec §6.1）。

* **host:** file.readdir（ICU collation 对齐 localeCompare） ([7d062df](https://github.com/sekfung/ESCode/commit/7d062df848a6d621fd0d2816fb1cf2bc0cb189f7))
  * 语义：lstat（Dirent）语义 + 隐藏项过滤（`includeHidden !== true` 时丢 `.` 前缀）+
  * 排序用 ICU4X（新增 `collation.rs`）复现 ICU 默认 collation：`localeCompare` 的默认
  * 分文件：`domains_file_readdir.rs`（原 `domains_file.rs` 超 400 行边界）。

* **host:** gen-ui 域迁到 Rust Host（5/6，prepareSandbox 留 Electron） ([8623174](https://github.com/sekfung/ESCode/commit/8623174fda9f401fa70d78ced36783eef17a7415))
  * readDocument 是安全边界：前缀 → 逐级拒符号链接 → 再看前缀 → 常规文件 →
  * 状态存储三层串行化：scope 级异步锁、跨进程文件锁（复用
  * updatedAt 严格单调（同毫秒连写 +1），容量淘汰保留最新 100 条。

* **host:** git 域迁移到 Rust Host（17/18 方法，TS oracle 差分） ([4fff11d](https://github.com/sekfung/ESCode/commit/4fff11dcaed342bc68c303256a5e649ec4a31bf0))
  * host-services/git：GitService（Git CLI 唯一事实源，唯一进程态为目录折叠的 repoRoot 集合）；
  * host-rpc：`git` channel；host-proto IMPLEMENTED 登记 17 个方法。
  * generateCommitMessage 登记为缺口：依赖 providers 域的首选模型与运行时 generateText，
  * 验收：packages/desktop/test/rustHostGit.test.ts 两份同构夹具逐步 toEqual；

* **host:** git-checkpoint 域迁到 Rust Host（4 个成员） ([893b3dd](https://github.com/sekfung/ESCode/commit/893b3ddbf80cb52fced17bf1694e1051af1e0246))

* **host:** git.generateCommitMessage 迁到 Rust Host（git 域 18/18） ([f3b129f](https://github.com/sekfung/ESCode/commit/f3b129f6ece9e6e7e15c6e221231696f2a78946f))

* **host:** initializeWorkspace 预热（就绪等待 + 协议常量，失败降级） ([96ed5d4](https://github.com/sekfung/ESCode/commit/96ed5d4e3382c53c1d3b42597bcfc12838276fd9))
  * host-runtime：实例读循环记录运行时报到（startup/storageState phase=ready），
  * escode-session.initializeWorkspace：建/复用实例后等报到，返回 {available, workspaceKey,
  * 测试：Rust 端到端（预热后实例可见、协议常量、dataDir 不可用降级）+ 真实客户端断言

* **host:** M3 automation 本地管理面 ([0041f99](https://github.com/sekfung/ESCode/commit/0041f99ead5b3d2d74d5fcb588decd641676e871))
  * list/listAll/delete/setEnabled/listRuns/deleteRun 6 个成员
  * 与 TS 共用 automations/automation_runs 表与投影
  * create/update/restart/runNow 仍为显式缺口（cron/派发器）
  * spec: rust-u2-automation.md；rpc_automation e2e
  * 拆分 agent_v4 connection/automation 方法表守住 400 行

* **host:** M3 bots 配置/状态/绑定子集 ([db498dd](https://github.com/sekfung/ESCode/commit/db498dd4689392a6d1119cffbafc5f210fb8cd71))
  * getConfig/saveConfig/listBots/saveBot/removeBotSecret/deleteBot
  * getStatus/getBotStates/resetBotState/createBindCode/listWorkspaceRefs
  * syncAppRuntimePreferences no-op（Host 无 Bot 远端 runtime）
  * 平台 provider/入站/群投递/V4 流 20 个成员仍为显式缺口
  * spec: rust-u2-bots.md；rpc_bots e2e

* **host:** M3 hooks 域迁移（loadHooks/saveHooks/grantWorkspaceHookTrust） ([a89f020](https://github.com/sekfung/ESCode/commit/a89f020336c61076e12cf2e5259450f1d04a1bfb))
  * 新增 hooks 服务域：项目/用户/legacy 发现、canonical digest、trust store 只读与 fail-closed、原子写
  * Host 通道实现三个成员；grant 转发运行时 workspace/hooks/trustGrant
  * 契约清单翻 Implemented；真实客户端差分 L4、持久化 L5、存量读入与 grant 转发用例
  * spec: docs/specs/rust-u2-hooks.md

* **host:** M3 off-peak-task 本地管理面 ([2066cf9](https://github.com/sekfung/ESCode/commit/2066cf9e36c45c35e15ec9db8f5c1edfd5592d4f))
  * list/get/update/pause/continue/cancel/delete/deleteHistory 8 个成员
  * 与 TS 共用 tasks-index.sqlite；终态守卫、History、selection issue 投影
  * createTask/额度/支持快照仍为显式缺口（依赖 Off-Peak server client）
  * spec: rust-u2-off-peak.md；rpc_off_peak e2e + 4 单测

* **host:** M3 plugin-sync 与零散成员收口 ([7d850df](https://github.com/sekfung/ESCode/commit/7d850dfc7e9172f757f0144704c1c2523665d462))
  * plugin-sync 7/7：候选/远端状态/归档导出导入/写权限/marketplace 源镜像
  * 归档 tar 格式与 TS 逐字节一致；gzip 容器差异已登记
  * escode-agent.listSessionSubagents、escode-session.getWorkspaceRuntimeIdentity
  * 边界项逐成员定性；conversation-share/marketing-touch 仍为阻塞项
  * spec: rust-u2-plugin-sync.md；scope-boundary §7

* **host:** mcp-sync 8 个方法迁到 Rust Host（sync 2/3） ([2a32c0a](https://github.com/sekfung/ESCode/commit/2a32c0a570a0834e56617c528e9c23628bc3bbb4))

* **host:** media-preview refreshPlaybackUrl/release；docs: M3 377/162 ([db67d8f](https://github.com/sekfung/ESCode/commit/db67d8fcc69636dd4ee09df8c0eefa250aa18024))
  * Host 不签发 host-range 租约：refresh 按过期返回，release no-op
  * 更新 M3 报告剩余项与库存计数

* **host:** memory 域 6 个成员迁到 Rust Host；domains 装配拆出 domains_simple ([05d6bc5](https://github.com/sekfung/ESCode/commit/05d6bc5b8393a20fbc264fae9f858b94924a62ef))

* **host:** onboarding-record 域 9 个成员迁到 Rust Host ([4d06566](https://github.com/sekfung/ESCode/commit/4d06566562aa59f2ab06671795e52b254db22f2c))

* **host:** output-style 域 7 个成员迁到 Rust Host ([c849b63](https://github.com/sekfung/ESCode/commit/c849b633f06091687908b9de32eccfa357a64c0b))

* **host:** P5 Built-in 物化 / local-url / 插件结果校验 / client-config·scenes / 跨窗口广播验证 ([d5f3566](https://github.com/sekfung/ESCode/commit/d5f3566cb9efe37192fc7f03a9d579f1d45dff79))
  * P5a：Bundled/Active/LKG 选择与锁内物化，Endpoint 隔离路径与 TS 逐字相同；A16b 的 provider 配置位置经 runtimeEnv 下发
  * P5b：Remote 下载（统一出口）、刷新租约与退避（控制文件格式与 TS 共存兼容）、Endpoint 随设置覆盖动态解析
  * provider-provisioning-target.apply 登记为非缺口（远程环境 target，信封依赖已删除的账号体系）
  * Host→Main 授权请求/应答（localMediaPreviewAuthorize/Authorized）+ pending 表 broker；rpcListen.localMediaPreview 声明能力
  * refreshPlaybackUrl/release 属远程 SSH 代理，不是 Host 缺口
  * listPlugins 超时后回收管理面实例并重试一次（RequestTimeout 类型化错误）
  * 全部管理面方法与引用目录的结果 schema 校验（声明式 DSL，TS zod 语料 13870 条对拍，--check 并入 check:escode-host）
  * Host 自己的 ApiClient（Endpoint 改写、来源头、request-id、超时、ApiError）
  * client-scenes.list、client-config.getSnapshot、getDynamicWorkflowClientConfig（缓存/在途合并/fail-closed）
  * getOffPeakClientConfig 不迁（off-peak-task 已移出范围）

* **host:** plugin-management 域 18 个成员迁到 Rust Host ([0f6d7dd](https://github.com/sekfung/ESCode/commit/0f6d7dd4ca70bab6efad0650c68f2914c6ba7109))

* **host:** prompt-attachment-transfer（本地）与 media-preview.prepare（内联）迁到 Rust Host ([9d0107c](https://github.com/sekfung/ESCode/commit/9d0107c915963f1483dccc5c3332c47bed6d7063))

* **host:** provider runtime headers 反向请求/应答 ([d2ffa8a](https://github.com/sekfung/ESCode/commit/d2ffa8a74763a9f8810bd6f3600d044578498b5d))
  * runtime interaction/requestProviderRuntimeHeaders -> Renderer 事件
  * respondProviderRuntimeHeaders 把 UI headers 合成为 requestAuth 写回
  * escode-agent/escode-session 两个通道；无订阅者 fail-closed、120s 超时
  * 取消事件仍 Pending（已登记差异）

* **host:** provider-settings.resolveModelConfig 迁到 Rust Host；修 rpc_task_sync 事件断言的潜在竞态 ([9be583e](https://github.com/sekfung/ESCode/commit/9be583ecc2d0b3a3e685559832374154fe73feeb))

* **host:** provider-settings.testModelConnectivity；修正 media-preview 差异归属；修 controller.rs 超行数门禁 ([cb98c17](https://github.com/sekfung/ESCode/commit/cb98c17dc3508b13d17f729a76c5fb9739fa2973))

* **host:** providers P1——model-selection.getView 迁到 Rust Host（TS oracle 差分） ([875ab30](https://github.com/sekfung/ESCode/commit/875ab3024448a470f0121ba04d0b556339ea8152))
  * host-services/provider：从 Built-in + Personal 两份文件解析 Registry（Provider/Template/Account 叠加、
  * host-rpc：`model-selection.getView`；配置路径由 Main 经 rpcListen.runtimeEnv 提供，缺失回显式缺口。
  * 验收：真实 config/provider/escode-builtin.json + 15 种 Personal 场景 × 17 种入参与 TS ProviderRuntime 逐项全等；
  * 登记差异 9 项见 docs/specs/rust-u2-providers.md §6（叶子校验粒度、正则方言、option map 判定等）。

* **host:** providers P2——provider-settings.getView/refresh 迁到 Rust Host（TS oracle 差分） ([79c23e4](https://github.com/sekfung/ESCode/commit/79c23e47ca707b73197feba7b4cf4dcbce52dd48))
  * resolve 同时产出全部已解析 Provider（含不可执行者）与 Registry；新增 issues（必填缺失/空白 key/非法 baseUrl/缺失模板，
  * host-rpc：model-selection 与 provider-settings 共享同一个 ProviderService（provider_channels）。
  * 验收：与 TS ProviderRuntime.providerSettings 在 P1 的 15 种 Personal 场景（含 5 类带问题的 Provider）逐项全等；

* **host:** providers P3——provider-settings Personal 写入面 10 法迁到 Rust Host ([9460a8b](https://github.com/sekfung/ESCode/commit/9460a8b38a6bbe314df823eaaaa4c3d47aa784f9))

* **host:** providers P4——model-selection / provider-settings 的 onDidChange 迁到 Rust Host ([6d510a1](https://github.com/sekfung/ESCode/commit/6d510a11252c895fe4a0669fde5e5e187fbd5884))

* **host:** Renderer RPC 面（@escode/rpc 协议）+ ESCODE_RUST_HOST dev 开关与 Main 接线 ([d1bffad](https://github.com/sekfung/ESCode/commit/d1bffad41ff920644d6134ac7a7d59b64bc33433))
  * 13 字节帧（type/id/ack/length）+ VSBuffer 序列化（VQL、DataType 标签、Object JSON）+ channel 分派；
  * HTTP 契约对齐 server-core：/api/server-info、POST /api/rpc-host-capability（一次性 TTL）、
  * setting channel 接设置域（get/update，含 ProxyChannel.toService 的实参数组打包语义）；
  * Host 进程新增控制消息 rpcListen/rpcListening
  * packages/desktop/src/main/rustHostBridge.ts：startRustHost（spawn + hostReady→rpcListen→rpcListening
  * packages/desktop/src/main/rustHostManager.ts：App 级懒启动（single-flight）、attachment 端口生产、收口
  * webRemoteControlSharedHostAttachments 本地分支接入（消费者只用 attachmentId 与 port，已核对）；

* **host:** serial 端口枚举与轮询（S1：list / setWatching / onPorts） ([bb3d35a](https://github.com/sekfung/ESCode/commit/bb3d35a23f646f4723c394652eb58b23fece8dc3))
  * 枚举：Linux 扫 /sys/class/tty，只收有 device 子目录的（虚拟 tty 没有）且 /dev 下
  * 轮询：1500ms，只在 watching 时跑；setWatching(true) 清空 dedupe 键并**立即**轮询
  * 作用域：连接级 channel（with_scoped），每连接一个服务实例，落实「每连接≈每窗口」。

* **host:** serial 会话与自动重连（S2：open/close/getSnapshot/listSessions/onStatus） ([f1a57e1](https://github.com/sekfung/ESCode/commit/f1a57e102aeca57185d235e3a0e088d689c1f068))
  * 传输层走 Linux termios（libc）：open(O_RDWR|O_NOCTTY|O_NONBLOCK) → cfmakeraw →
  * 状态机 closed→opening→open→closing→closed，以及 open→disconnected→（设备回来）→
  * 断开检测改用**轮询**（S1 的枚举已在手）：open 中的会话路径从枚举里消失即判断开，最多比
  * 保留 8 个会话条目，超出按 lastChangedAt 淘汰最早的已关闭会话。

* **host:** serial 收发缓冲（S3 前置件） ([0cdfef2](https://github.com/sekfung/ESCode/commit/0cdfef22a63f84b14a70cc0319e186877facdeb9))
  * seq 由连接分配（跨会话/跨关闭重开单调递增），clear 不回退；
  * 按字节上限从最旧淘汰，且至少保留最新一条（否则单条超限时 seq 会原地踏步）；
  * evicted_through 记录已淘汰的最大 seq（游标早于它即有缺失）；
  * sessionId 只在 source === "agent" 时保留；onData 载荷 = {...chunk, path}。

* **host:** serial 数据面（S3：write / clear / onData） ([afdabad](https://github.com/sekfung/ESCode/commit/afdabadcd6498873f06982290bad10b3ff71e74e))
  * 读路径按用户决定走 AsyncFd + epoll：dup 出来的 fd 建 AsyncFd（tokio 1.53 用
  * 写路径：非阻塞 fd 上 EAGAIN 以 1ms 重试（上限 5s）；**先记 TX 再真正写出**，
  * RX 合并（data.rs）：尾部静默窗口 10ms 成段，pending 超 4096 字节立即成段；
  * RPC 层 param_bytes 解码 @escode/rpc 的嵌套 Uint8Array 标记

* **host:** serial 信号与循环迁到 Rust Host（S4，14/14） ([8ff2d68](https://github.com/sekfung/ESCode/commit/8ff2d68a3e059c2f6fd910e80c2a1e82c76ca37d))

* **host:** setting.ensureDefaultProject / updateDataBaseDir 迁到 Rust Host ([c3ccc51](https://github.com/sekfung/ESCode/commit/c3ccc51e9c0048f163a56e0601f97dd2f95a7491))

* **host:** settings-sync 发现/导入引擎迁到 Rust Host（sync 19/19） ([b21c80a](https://github.com/sekfung/ESCode/commit/b21c80a7d5710b3e93bee7e4c6815271c1d8e439))

* **host:** settings-sync 迁移 4/6（AGENTS.md 迁移与首启状态，sync 3/3） ([1207ba5](https://github.com/sekfung/ESCode/commit/1207ba5bf21cec7fafb2941053044f35bc90fd33))

* **host:** skill-sync 5 个方法迁到 Rust Host（sync 1/3） ([8a3fd37](https://github.com/sekfung/ESCode/commit/8a3fd37f2e2f0d7da06aa5bc7a888b7148627e16))

* **host:** subagents 目录解析与状态写入面（4/8） ([01c4aa7](https://github.com/sekfung/ESCode/commit/01c4aa75623a509f02d9d7580e9f0005365db7d6))

* **host:** subagents 写三法迁到 Rust Host（8/8 收口） ([6715849](https://github.com/sekfung/ESCode/commit/67158493b6e708cc9a1b0e2e83838b90f3fe0f81))
  * 写前回解析：序列化结果必须能被 parseSubagentMarkdown 重新读出，否则报错不落盘
  * create 用 create_new（TS 的 flag "wx"）原子创建，文件已存在即失败；
  * 改名必须迁移 disabledAgentIds：id 含 name，不迁会让刚禁用的 agent 因改名重新启用。

* **host:** subagents markdown 解析器（list 的前置件） ([a1b13f7](https://github.com/sekfung/ESCode/commit/a1b13f78ecb3dd95ee19d7d63e202bb8ec0b143e))
  * frontmatter 切分：去 BOM、首行必须是 ---、取紧随其后的第二个 ---（CRLF 也接受）；
  * 宽松解析：空行/注释跳过、`- ` 列表项、空值开启列表、缩进续行视为块状 mapping、
  * 数值按 i64（JS 序列化回 42 而不是 42.0，这一条同时修好了 maxTurns 的 as_u64 失配）；
  * mcpServers 四态（未配置/裸值/块状 mapping/合法名字列表）——只有最后一种能扩大
  * profile：缺 frontmatter/name/description 失败且文案逐字，memory 非法是带 profile
  * model：inherit 家族不产生覆盖、custom: 编码解码、$ 在首位或末位不当档位。

* **host:** subagents.list 迁到 Rust Host（5/8） ([b8469eb](https://github.com/sekfung/ESCode/commit/b8469ebdd3edbec3c0b73cc7fe70e4f193ec5b6e))
  * Settings 侧摘要与 runtime 侧刻意不同：tools 不剥 (...)、项目级不丢 permissionMode、
  * 文件发现只扫第一层（插件才递归），禁用态只作用于 user scope；
  * 插件发现用另一个分组版缓存扫描器（版本目录数字感知降序、过滤瞬态项），
  * 插件 agent 名为 <pluginName>:<bare>，裸名唯一时额外产出一条运行时别名，否则报

* **host:** system 域 info()（优先序 4 起点之一） ([07707b3](https://github.com/sekfung/ESCode/commit/07707b3c67bf0d80443f8c33da7d6f8aa8a13b87))
  * host-rpc/domains：system channel —— info() 返回 {homedir, platform}（homedir 取 Main 经 rpcListen
  * 测试：Rust 端到端（homedir 与 platform、方法缺口）+ 真实客户端断言 systemService.info()

* **host:** system.listIntegratedTerminalShells / probeIntranet 迁到 Rust Host ([5781df1](https://github.com/sekfung/ESCode/commit/5781df1150be6218dfb7cf85ab1d660d55dec39e))

* **host:** tasks-index 存储层（迁移账本对账 + getTaskMeta 投影） ([5fdd5f9](https://github.com/sekfung/ESCode/commit/5fdd5f99ab9cbee03ffffb566474000006d3c83e))
  * 生成器 `scripts/generate-escode-host-tasks-schema.mjs`：用真实 TS
  * `tasks_index` 模块：打开既有库（`busy_timeout=25`、`foreign_keys=ON`，与 TS 同）；
  * `getTaskMeta`：TS `TaskIndexRepo` 同序投影（meta_json 优先、标量列覆盖身份与

* **host:** tasks-index 生成器接入 CI 门禁 + 子 spec 进度登记 ([80d9c3e](https://github.com/sekfung/ESCode/commit/80d9c3e3c0838cc866a77e71d34235934ea01c53))
  * `pnpm check:escode-host` 加 `generate-escode-host-tasks-schema.mjs --check`：
  * 生成资产加 `#[rustfmt::skip]`：生成文本固定，`--check` 不再被 rustfmt 换行差异干扰。
  * 修 2 处 clippy `nonminimal_bool`（`is_some_and(!x.is_some_and(f))` → `is_none_or`）。
  * 子 spec `rust-u2-tasks-index.md`（优先序 5 域）：所有者与数据流、存储契约、接口映射、

* **host:** tasks-index 写层（writeRecord/syncTaskMeta 合并语义） ([ccafcbc](https://github.com/sekfung/ESCode/commit/ccafcbc26d730a01d880c0321cde55d4c3ecf27f))
  * `writeRecord` 的 upsert 逐句对齐 TS：`unread_at` 的 CAS marker、`last_unread_at` 取
  * `syncTaskMeta` 的合并规则：`updatedAt = max(入库, 已有)`（快照可能落后于 agent patch）、
  * `seedTaskMetaIfMissing`：存在即原样返回（不覆盖）。
  * 测试夹具抽 `test_support.rs` 共用；写用例独立 `write_tests.rs`（400 行边界）。

* **host:** terminal 域迁移到 Rust Host（6/6 成员，TS oracle 差分） ([699b34d](https://github.com/sekfung/ESCode/commit/699b34da3ff81708ebea2c36d412bc63cd0b88b5))
  * host-services/terminal：TerminalService（Host 内唯一，终端属于 Host 不属于连接）；
  * host-rpc：`terminal` channel（4 方法 + onDynamicData/onDynamicExit）；host-proto 登记 6 个成员。
  * Alacritty 与 macOS 检测器登记为 T2 差异；Windows 路径未在 Windows 上验证（spec §6）。
  * 验收：rustHostTerminal.test.ts 同一脚本化 SHELL 驱动 node-pty 与 Rust 全等；

* **host:** usage-stats.getAppUsageSnapshot 迁到 Rust Host ([53b122f](https://github.com/sekfung/ESCode/commit/53b122fc61a49170d072980db22f52afaa757106))
  * `Instances::live_any()`：取任意活实例的策略归实例表所有者（新增，带测试）；
  * existing-only：无活实例即报 `no_active_workspace`，不为一次查询拉起运行时；
  * 请求体只留 `{range, timeZone}`（TS 字面量整形），不带 `workspace` 引用。

* **host:** window-controller 本地子集迁移到 Rust Host（8/8，TS oracle 帧差分） ([4d665d3](https://github.com/sekfung/ESCode/commit/4d665d30c8f949343db01683e1bffd661df810c9))
  * 投影与订阅面：`subscribeControllerV4`/`resyncControllerV4`/`unsubscribeControllerV4`/
  * `listTaskList`：本地 source 的 membership 过滤、搜索分支（索引 DB 全文搜索 + 投影叠加）、
  * `mutateTask` + `deleteArchivedTask(s)`：复用 `escode-task` 同一写面
  * TS oracle 帧差分：`scripts/generate-escode-host-controller-fixtures.mjs` 用真实 TS 投影
  * 端到端：真 WebSocket + 真索引 + 真会话（`tests/rpc_window_controller.rs`、
  * 负向验证三处（均已复现并回滚）：membership 过滤改坏 → 单测失败；去掉 removals →
  * 门禁：cargo fmt/test/clippy(-D warnings)、`pnpm check:escode-host`、`pnpm test:escode-host`

* **host:** workspace 展示面读转发（readWorkspacePresentation，优先序 4 起点） ([8a4c71b](https://github.com/sekfung/ESCode/commit/8a4c71b496ffb8827fa15b8b4d0ccb3460778d54))
  * forward.rs 增 runtime_call_with（转发前注入额外字段）
  * escode-session.readWorkspacePresentation：先 runtime/capabilities 判执行能力位，
  * 已登记差异：TS 读取前同步账号 provider 配置（providers/account 域未迁移），
  * 测试：Rust 端到端（展示面含 mode/slashCommands/执行能力位）+ 真实客户端

* MCP 资源订阅（runtime 登记 + live-only delta + Host 成员） ([676dd24](https://github.com/sekfung/ESCode/commit/676dd245d3d10c3b22af30f3d6ac31c9c45c08c7))

* MCP Apps 采样（账本 + 任务模型 + Host 成员） ([c384c53](https://github.com/sekfung/ESCode/commit/c384c53ce9b395b8ce8a8a614c67688a7625ed68))

* **rea:** 以内置插件接入 REA 逆向分析（宿主注入，与 serial/circuit 同构） ([8e83ca6](https://github.com/sekfung/ESCode/commit/8e83ca69ea2d58749b99e9dc8d560e9c5a35b64b))
  * 插件包 apps/escode-cli/packages/rea-plugin：src/server.mjs（构建产出 dist/mcp/server.js，与
  * 宿主注入：built-in-rea.ts 与 mcp_rea.rs 复用同一套启动器；Rust 的 host_plugin_server 增加
  * 运行时不经插件 seed：约 100 MB / 6,555 文件，seed 每次启动都要 walk+sha256、首次还要整棵复制
  * 免审批面 113/170：由 scripts/generate-rea-auto-approved-tools.mjs 从已安装版本生成（读语义前缀 +
  * 可选引擎（Ghidra/Hopper/IDA/JDK/WABT/tshark/JEB/ILSpy/浏览器）不随包：manifest 声明 userConfig，
  * 诊断：入口把 REA 的 stderr tee 到插件数据目录并写运行时自检报告 —— Rust 宿主丢弃 stderr，
  * codec 帧上限 8 → 16 MiB：REA 按 10 MiB 预算产出结果且不可调低，旧上限会在 8–10 MiB 窗口
  * SEA 与远端本阶段排除（前者已加防回归断言），桌面随包；分发范围与理由见 spec。
  * scripts/upgrade-rea.mjs 一条命令完成 pin/运行时/技能/生成物同步；rea-upstream-watch workflow

* **remote:** 远程环境默认使用 Rust runtime（musl 静态链接），不回退 Node ([4d7862c](https://github.com/sekfung/ESCode/commit/4d7862cc9f9cb450173527e077cde9fbfd58e4f2))
  * 打包：远程 release 的 glm 组件携带 escode-cli-rust（scripts/remote-rust-agent.mjs）。
  * 校验：构建期必需条目、afterPack 断言与运行时本地 release 完整性都包含该二进制
  * 部署：上传为 ~/.escode/server/agents/glm/escode-cli-rust 并设为可执行；远端缺该文件时
  * 选择：桌面安装包的 Host 给远端 server 注入 ESCODE_REMOTE_PACKAGED_RUNTIME=1，远端视同
  * 打包流程未指定平台时只构建 linux-<目标架构>
  * CI：rust-runtime 增加两个 musl target；desktop job 依赖其产物并经
  * 同步 rust-packaging / offline-remote-assets spec，补充单测与部署回归用例

* **rust-wiki:** LLM Wiki 引擎（存储、项目身份、多格式导入、检索、治理） ([69ceed6](https://github.com/sekfung/ESCode/commit/69ceed63370aaad0c12a39e5a04bdb0c01551943))
  * domain::wiki：条目 schema/校验、frontmatter、分词与倒排索引、BM25 排序与预算、
  * 新 adapter crate escode-cli-wiki：原子写与 scope 锁、Git/worktree/remote 项目身份、
  * core-api：WikiPort 端口
  * 边界检查把 wiki 列为 adapter crate
  * 规格：apps/escode-cli-rust/docs/llm-wiki/

* **rust:** 阶段 0 护栏与测量基线（统一 Host 计划，docs/specs/rust-unified-host.md） ([1d94fb3](https://github.com/sekfung/ESCode/commit/1d94fb38de1bb3957d33a5bf329b7c04ebc186bd))
  * rust-toolchain.toml pin 1.95.0；workspace clippy deny 20 条 + clippy.toml 测试放宽；
  * 17 个越界文件按域拆分（≤400 行）；wiki Windows \?\ realpath 修复；P0 四项完成：
  * 修复 Windows 主线程 1MB 栈溢出（0xC00000FD）：入口改 16MB 栈线程运行 tokio runtime
  * 全套件验收：407 用例 0 失败（CI 口径串行）；ESCODE_TEST_EXIT_MS=15000 关闭预算加固
  * scripts/diff-escode-cli-rust-parity.mjs（+capture/diff/report）：Node 捕获 → 双跑差分 →
  * 我方差异账本第一轮 docs/reports/rust-difference-ledger-2026-10-14.md（M/T/C/S/A/F）
  * M-1/M-2：Chat 请求体不再硬编码 max_tokens（option map 决定、回退 TS/AI SDK 默认
  * C-1/C-2：memoryEnabled 进全部遥测事实与后续命令 ack（telemetry-facts 差分用例
  * T-3 归阶段 2 P2（消费方=ARMS 埋点，需 MCP 进程账本）；T-4 归投递层对齐
  * scripts/bench-escode-app.mjs（+sampling）：三配置 A/B/C、交错 N 轮中位数、双口径内存、
  * docs/reports/rust-app-perf-baseline-2026-10-14.md：A 1419.6 / B 1293.6 MiB 工作集

* **rust:** 阶段 1 runtime 库化（embed 入口/双传输/显式环境/实例作用域） ([d47d751](https://github.com/sekfung/ESCode/commit/d47d7516984f7da913872243f091c4d525d7c009)), closes [#5]() [#7]() [#8]() [#11]() [#6]()
  * embed::spawn(RuntimeOptions) -> Embedded{io,task,cancel,tasks}（形态对标 ZCode-rs embed.rs）
  * core-api 新增 env（EnvSnapshot/InstanceContext，workspace 唯一 std::env 读取点）与 scope（Spawner：
  * app-server 抽出 Link/StdioLink/MemoryLink + 泛型 start_io（两链路共用同一段读写代码）
  * owner 锁进程内登记表 + 实例 token（WorkspaceOwnerGuard），exit 75 语义不变

* **rust:** 阶段 2 u2a 骨架（apps/escode-host：有界通道/原子写/Gateway/控制通道/契约清单） ([d1046f7](https://github.com/sekfung/ESCode/commit/d1046f70361c0ae7ab94ff356fb1cb06c4511021))
  * host-core：bounded（命名队列 + 溢出策略枚举 + 丢弃/背压指标）、atomic_write（fsync + rename 重试 32/33 等 + 父目录 fsync + 失败清理）
  * host-proto：服务契约清单（channel/method + Implemented/Pending）与 --check 三向对账（重复/未登记/清单外）
  * host-gateway：register 同步取号（lane 序 = 到达序）、每连接一个写者、capability 握手 ∩ 支持面、未覆盖 method 回 NNotImplemented
  * host-control：typed ControlMessage、控制与日志共用单有序队列（控制必达/日志满即丢）、结构化 ControlError code
  * TaskScope 与进程回收复用 escode-cli-core-api::scope / escode-cli-host（跨 workspace path 依赖验证通过）
  * 边界脚本 scripts/check-escode-host-boundaries.mjs（400 行、分层、0 处进程 env 读取）+ pnpm check:escode-host / test:escode-host
  * u2 子 spec 记录实施进度与余项（进程入口、Main 侧 dev 开关、CI 作业接入随 u2b）

* **rust:** crates/net 网络出口单入口（P1 [#6](https://github.com/sekfung/ESCode/issues/6)） ([55fe5b8](https://github.com/sekfung/ESCode/commit/55fe5b88239c6190eeba1bcff2d9cf8699ec4ddd))
  * 新 crate escode-cli-net：Purpose{Model,Mcp,WebFetch} 决定未命中代理时的默认（模型沿用 reqwest 变量；
  * 三处出口改走 net：model/provider.rs、tools/mcp_http.rs（+ mcp_oauth_http 改 async）、tools/web_fetch_http.rs；
  * 强制：workspace clippy disallowed_methods=deny（reqwest/reqwest_mcp 的 Client::builder 只允许在 net 内）；

* **rust:** Host 进程入口与控制通路（attach/转发/扇出/收口端到端） ([330e3af](https://github.com/sekfung/ESCode/commit/330e3aff7d320e227aeabec5c26a1e856d391f54))
  * escode-host 二进制：stdio NDJSON 控制循环（hostReady 握手、windowAttach/Detach/Request、
  * host-control 协议扩展：WorkspaceDescriptor、WindowAttached/WindowRequest/WindowFrame、
  * host-runtime 读写分离：Instance 的读端（帧泵独占）/写端（请求并发）各持一把锁——
  * CLI app-server 增 EmbeddedIo::split()（读端/写端），嵌入链路支持并发读写
  * 测试：host 入口端到端 2 例（真实子进程：attach→代次→session/list 往返→多窗口复用同代次→

* **rust:** Host runtime-link 与实例表（embedded/child 双链路、懒重启 generation、single-flight 收口） ([e3e1535](https://github.com/sekfung/ESCode/commit/e3e1535549b103f53912502cc793212c736e4db8))
  * host-runtime/link.rs：Link 选择器（Embedded 默认 / Child 子进程 app-server --stdio），
  * host-runtime/lib.rs：Instances 实例表——每 workspace 一实例（身份 workspaceIdentity ?? path）、
  * 测试 5 例：双 workspace 独立、懒重启代次、single-flight、收口无残余、child 链路同就绪帧

* **rust:** MCP 客户端事件总线 S1 ([9d55c98](https://github.com/sekfung/ESCode/commit/9d55c98c2b15db31835e73559ceec0d4a193177f))

* **rust:** mcp/ui* 实例、凭证与资源读取基础 ([dcf2542](https://github.com/sekfung/ESCode/commit/dcf2542354b177dd5c7d3f1f908e9d5e134df28f))

* **rust:** P1 [#11](https://github.com/sekfung/ESCode/issues/11) 剩余项（策略表 serde 形状 + registry 类型化与完整性断言） ([6f49f19](https://github.com/sekfung/ESCode/commit/6f49f193c1f0d69f155ae22725f13d761567d3bb))
  * bash_policy_tables.rs：bash_policies.json 的 serde 形状，deny_unknown_fields；
  * bash_registry.rs：fig 命令树类型化为 HashMap<String, Node>（位置数组 [names,options,argFlags,children]
  * bash_rule_prefix.rs：前缀遍历全部改 typed 访问（node[1]/[2]/[3] 下标清零）

* **rust:** Renderer 传输面（WebSocket + capability 握手 + 实例扇出，端到端） ([b16840c](https://github.com/sekfung/ESCode/commit/b16840c49afe3637a7ceec1a81c677343674f1c7))
  * host-surface：WS 监听 + Hello/Ready 握手（capability 与 Host 支持面求交，未覆盖回 -32601）、
  * Host 进程：控制消息 surfaceListen/surfaceListening 开关传输面，Shutdown/EOF 收口
  * 测试 4 例：握手与路由/缺口显式、多连接共享同实例并扇出、空能力只收错误、

* **rust:** settings 域（双 Host 交替写读演练通过）+ 契约清单源码对账 ([68c197e](https://github.com/sekfung/ESCode/commit/68c197e4c7ba82b4f171fcf29943d21d5e49551c))
  * host-services::settings：与 TS Host 同文件同格式（<home>/.escode/v2/setting.json）——
  * 控制协议 settingsRead/settingsWrite/settingsValue；Host 进程实现该域
  * 互操作演练 packages/services/tests/escode-settings-host-interop.test.ts：真实 TS 设置服务
  * 契约清单与源码对账：control/settings 通道标记 Implemented，新增漂移测试断言

* **rust:** v4/command schema 校验（P1 [#10](https://github.com/sekfung/ESCode/issues/10)：TS zod 语料 + 子集校验器 + 漂移门禁） ([e3b864a](https://github.com/sekfung/ESCode/commit/e3b864a389356003682f56bae3b16f7089d48916))
  * 生成器 scripts/generate-escode-cli-rust-command-schema.mjs：从 packages/shared 的
  * crates/protocol/src/json_schema.rs：JSON Schema 子集校验器（type/const/enum/properties/
  * crates/protocol/src/command_schema.rs：validate_command 对齐 TS parseCommandEnvelope
  * 协议入口（app-server stdio dispatch）记录违规为 debug 日志，不改变线上应答

* **ui:** 生成速度拆分为实时接收速率与轮末解码速度 ([c44f005](https://github.com/sekfung/ESCode/commit/c44f0050d84dca74efc0aa4e934b2364b9e9593d))
  * 轮末解码速度改为 CLI 投影持有的会话事实：runtime 在流消费处记录每次调用的首个与末个非空生成
  * 口径 = Σ(outputTokens − 1) / Σ(首个生成 delta → 末个生成 delta)。相对 dsh 修了四处：终点取末个
  * Renderer 收缩为只做实时接收速率，轮末直接读权威字段；删掉本地阻塞区间启发式、缺口不留平均值、
  * 文案区分两个量：实时"实时生成速度"，保留"上一轮解码速度"。

* **ui:** 知识库作为工作区主视图接入侧边栏与任务导航 ([abe0b87](https://github.com/sekfung/ESCode/commit/abe0b871251e87d5cc878ac3b9964282a726afe2))
  * WorkspaceMainView 增加 "knowledge"；WorkspaceShellLayout 在该视图下隐藏 WorkspaceHeader，
  * 侧边栏新增「知识库」入口：桌面/本地显示、远控不显示，带 aria-pressed 与
  * 导航历史记录 workspace 身份（taskNavPushKnowledge 带上 workspaceAbsPath 与 workspaceIdentity）：
  * 设置页不再出现 knowledge 分区，保留 id 只用于迁移历史偏好与旧深链 fallback。

* **usb:** 抓包后端收敛为 Windows 内置服务，插件与抓包引擎按 spec §4 对齐 ([bfd8d19](https://github.com/sekfung/ESCode/commit/bfd8d1939eb20c8795107abe2369b0e1513c884e))
  * 后端收敛（backends.ts）：删除 dumpcap / USBPcapCMD 两条回退，`CaptureBackendId` 只剩
  * 离线指引（probe.ts）：新增 `offlineOnlyGuidance()`。Linux/macOS 的 `usb_probe` 给出平台级
  * 中断收尾（capture.ts）：抓包器收到外部信号时会优雅收尾、把产物留在 `<output>.partial` 并
  * 视图（analysis.ts）：时间戳一律按相对首条报文的偏移渲染，墙钟时间戳的抓包（USBPcap）不再
  * 抓包引擎（Rust）：`MON_IOCX_MFETCH` **不消费**环形缓冲（内核 mon_bin.c 的注释），取完必须用
  * 特权步骤（Rust）：`chown`/`chmod` 会跟随符号链接，`/dev/usbmonN` 若被换成指向 `/etc/shadow`
  * 其他：删掉无人使用的 `max_event_bytes`；`examples/write-sample.rs` 改成结构体驱动的可复用

* **wiki:** Desktop import dialog with scope choice, promote to global ([3865748](https://github.com/sekfung/ESCode/commit/386574814413cefbd6a22f9834c6f4e4bbfc0c7f))
  * Knowledge page: "Import documents" in the header (and in the empty state)
  * entry detail: "Promote to global" for live project entries (confirmed),
  * one manage() write path refetches after every write
  * tests: wikiImport helpers, entry actions, static dialog rendering,
  * spec/usage/report updated; desktop-only (no mobile adaptation)

* **wiki:** Desktop knowledge graph settings page ([c1d1e13](https://github.com/sekfung/ESCode/commit/c1d1e1397294a4fb0ce6898c591fe04db35d0728))
  * Settings → Knowledge Base section (desktop and mobile web) backed by
  * deterministic force-directed layout with label collision separation;
  * filters for scope/status/kind/sources, entry detail with provenance,
  * service maps -32601 to supported=false so non-Rust runtimes show an
  * spec §11 state ownership and event order; en-US/zh-CN strings
  * tests: layout/presentation/rendering unit tests, service protocol tests,

* **wiki:** global wiki at <storage>/wiki, project wiki versioned in the workspace ([1412e4e](https://github.com/sekfung/ESCode/commit/1412e4ec5a8b8a702bdd7883670859fc9d47fba9))
  * Global Wiki root is <storage>/wiki itself (storage = ESCODE_STORAGE_DIR →
  * Project Wiki lives in <project root>/.escode/wiki and travels with the repo:
  * directory, .gitignore and project.json are created on the first write only;
  * project.json pins the project id so clones on other machines (incl.
  * ingest excludes both wiki storages; wiki.projectStorage removed
  * spec §1/§3/§4, usage guide, delivery report and E2E fixture updated
  * tests: storage_layout (root, versioned layout, read-only, clone portability)

* **wiki:** integrate LLM Wiki into Rust runtime turns, tools and v4 protocol ([417ad45](https://github.com/sekfung/ESCode/commit/417ad45cd805b24263c6720723d7f2f9460f856d))
  * core: per-turn hidden wiki context committed through the session owner before
  * core: v4/wiki/status|graph|entry|manage engine requests
  * binary: compose WikiService into the runtime; `escode-cli-rust wiki` CLI
  * shared/services: zod schemas for v4/wiki/* and escodeAgentService methods
  * tests: runtime integration tests (injection order, dedupe, tool call,


### Bug Fixes

* 采样改为后台作业，取消不再被 actor 阻塞 ([4b853f6](https://github.com/sekfung/ESCode/commit/4b853f604431aaf0d4463f67cd4f5e0daf38e377))

* 对齐 MCP Apps 错误码（实例失效 -32000、资源 -32001） ([dc95f30](https://github.com/sekfung/ESCode/commit/dc95f303e9107244dcccbf2706acff0fdf2879af))

* 固定设置夹具语言偏好，避免 CI 系统语言改写 locale ([b6fbdaf](https://github.com/sekfung/ESCode/commit/b6fbdaf72e952b1d6bfccbfbf0dfbe1fced0eafc))

* 清理 Bots 删除后的测试与注释残留 ([5c144fb](https://github.com/sekfung/ESCode/commit/5c144fb15ae5a9cf9e76ec589672b5f861557009))
  * 移除 topic-resource/channel-reply 反向请求测试与已删除的 relay 导入
  * 更新 elicitation 测试、401 清理用例与 Bot 凭据夹具
  * 清理 Rust/UI/CLI 中指向已删除 Bot 能力的注释

* 升级 rustls-webpki 修复 RustSec 告警 ([704e2aa](https://github.com/sekfung/ESCode/commit/704e2aa0e9647f82017527b5badefeade56d8eaf))
  * 0.103.15 -> 0.103.16，修复 RUSTSEC-2026-0382 / RUSTSEC-2026-0383
  * 同步更新 CLI 与 Host 的 Cargo.lock

* 升级 webpki-root-certs 修复 RUSTSEC-2026-0383 ([b545268](https://github.com/sekfung/ESCode/commit/b545268c185c8d42c7dfda574fa1e8b680629f55))
  * 1.0.9 -> 1.0.10，移除对已泄露 emSign 中间证书的信任
  * 同步更新 CLI 与 Host 的 Cargo.lock

* 稳定 dev CI（workflow actor shell 继承与测试竞态） ([b50cb01](https://github.com/sekfung/ESCode/commit/b50cb014a0bb0243be1d1e9f67dde919507997d8))
  * Rust workflow actor 继承父会话 prompt 快照的 shell，不再用进程 SHELL/ComSpec 覆盖；与 TS workflow child 的 bashShellSelection 继承一致，修复 Windows Node/Rust parity。
  * rpc_task_sync 用 event_where 等带 background_terminal 的终态事件，避免取到中间 task_status_changed。
  * 确定性模型错误用例用 throw-after 闸门等两个 actor ask 都起飞，保证两条 cancelled。
  * B1 healthy 放大 streamIdle 超时，避免 CI 负载下误触发重试；B18 先等 wire grace 再取零请求基线。
  * GitHub marketplace 真实 clone 改为 ESCode_TEST_GITHUB_MARKETPLACE=1 opt-in，CI 只跑本地导入链路。

* 稳定 SVN 行锁定回调引用 ([06b5a97](https://github.com/sekfung/ESCode/commit/06b5a97630315591921059f6f45bbb350c9a31a8))

* 修复 CLI Rust 会话加载测试的共享 fixture 引用 ([b87e0f4](https://github.com/sekfung/ESCode/commit/b87e0f4781da92bf7d9dbec3fb719c54f9e50c28))

* 修复 SVN CLI context 的 Clippy 告警 ([f040dd4](https://github.com/sekfung/ESCode/commit/f040dd42efd3e7f726804a1eb59d377e9de125ed))
  * percent_decode 改为 let-chain 折叠嵌套 if
  * 分支标签改用显式 if/else，避免 obfuscated_if_else

* 移除 Rust telemetry 状态事件中的 usage 字段 ([c25d6f9](https://github.com/sekfung/ESCode/commit/c25d6f9b2227ec1afbcd5d23f4c8f0d9a11d2bfb))

* **agent:** owner 冲突如实上报并退避 spawn 重试 ([cb9b420](https://github.com/sekfung/ESCode/commit/cb9b42040f03779c84a2f8f3b1819ce03b778877))
  * 声明 supportsStorageStartup 的命令：getClient 等存储启动首个终态，ready 才返回 client；
  * owner 冲突按 workspaceKey 退避（1s→2s→4s，上限 5s），窗口内不再 spawn；
  * docs/specs/rust-packaging.md 补规则、时序图与验收；

* **agent:** Rust runtime 不再回退 Node，换代等待旧 owner 释放 workspace ([c770469](https://github.com/sekfung/ESCode/commit/c770469be4a0949a1bdc65ce6de522a250b547a3))
  * Host：startClient 先等待同 workspace 的退休进程退出（或其回收编排结束），再解析命令
  * Host：删除 rustRuntimeFailed 与二进制缺失的自动回退；默认 Rust 仅限桌面安装包，
  * Rust：owner 锁被占用时以 75 退出（WorkspaceOwned）
  * Rust：plugins/* 与插件作业按协议参数的 workspace 执行，不再与 runtime cwd 比对；
  * 同步 rust-packaging / rust-release-rollback / rust-plugins spec，补充回归用例

* App 工具结果复用 MCP 结果格式化 ([88b97bf](https://github.com/sekfung/ESCode/commit/88b97bf0de3a23b9ff99d44994d5f015466ec133))

* **bootstrap:** 对齐下线套餐后的 provider 语料与 Built-in revision 同步 ([c2ee17d](https://github.com/sekfung/ESCode/commit/c2ee17d3ca1cbbff28413d8ed1d6cb638b4d5397))
  * Built-in 刷新后若无账号服务同步 revision，Registry 会永久停在旧快照；在源变化时用 fail-closed 空账号对齐 revision。
  * 旧 CLI Personal Provider 导入、Built-in model runtime、协议 subagents/provider 回包、v4 冷恢复的期望更新到当前模板/规则/档位格式。
  * runtime-unbound-config 按 Registry fallback 规则断言新建空白会话的初始选择。

* **bootstrap:** v4 无桥边界收口；未投影 startNow 输入拦截旧 revision 命令 ([85f4012](https://github.com/sekfung/ESCode/commit/85f40120b43d6dd076a1fd5cac1ef189353669d0))
  * 把 model execution context 与 session residency finalization 迁到 v4 原生层，旧协议反向 re-export；v4 不再 import escode-protocol/。
  * startNow 输入已 admission 但 user row 未投影时，retry/edit/file rewind 返回 stale/proto.staleRevision，等投影推进后再执行。
  * read-pdf 测试改用当前 ModelConfig/ProviderRegistry 形状；marketplace 的真实 clone 探测使用 sanitize 后的子进程环境，不可达时显式 skip。
  * 标题 sidecar 的 x-zcode-session-type 期望改为 main（按宿主会话分类）。

* **bootstrap:** workflow child 继承父会话 shell；对齐插件/会话默认语料 ([038f9f5](https://github.com/sekfung/ESCode/commit/038f9f54f4ae23d56098488aa1269cd282ac0fc5))
  * workflow 子运行从父 runtime 读取已解析的 bashShellSelection，避免 prompt 的 Shell 与 Bash 实际执行分叉。
  * 官方商店可见插件计数、默认 usb MCP、GLM 模型 ID 规范化、startup/storageState 帧与 Agent 类型清单的期望更新到当前产品状态。
  * zsh 用例显式注入 shell 选择，不再依赖宿主是否安装 zsh 或 PATH 顺序。

* **ci:** 单测 job 补构建步骤；mcp_connection.rs 回到 400 行以内 ([5bb0f59](https://github.com/sekfung/ESCode/commit/5bb0f5903c3537b061eb1a79385c87c6e4e87c33))

* **ci:** 单测 job 的构建范围收敛到测试真正 import 的包闭包 ([a8d7864](https://github.com/sekfung/ESCode/commit/a8d7864660f7bea047094cb7cae3d58729cb27e2))

* **ci:** 等 Windows 旧实例退出后再启动 Host 切换演练 ([6b367dc](https://github.com/sekfung/ESCode/commit/6b367dc48cc8dca2817f85ac9c033c42edc28c1e))
  * 启动超时区分平台：Windows 90s，其余 60s
  * stop() 等待子进程 exit 后销毁管道；drill 每次启动前等待旧实例释放单实例/DB owner 锁
  * 超时错误附带子进程 exit/signal 与输出尾部，便于定位 Windows 卡点

* **ci:** 修复三平台 checks 首次暴露的 Windows 编译错误与 macOS typecheck OOM ([38a1441](https://github.com/sekfung/ESCode/commit/38a1441fb93e00a3f5fddfb175ee2036c6ac4992))
  * Windows：host-services gen_ui/document.rs 用了不稳定的 volume_serial_number/file_index
  * Windows：terminal/launch_tests.rs 的执行位断言整段放进 cfg(unix)，消除未使用变量（-D warnings）
  * macOS：全仓 tsc -b 在 7 GB runner 上堆 OOM（exit 134），只给 typecheck 步骤放宽到 5 GB

* **ci:** drill 隔离 userData 并修复 RUSTSEC-2026-0386 ([f5ef460](https://github.com/sekfung/ESCode/commit/f5ef4608dcfcfaa2718a56c2b9b154444d9eba54))
  * drill 设置独立 ESCODE_DESKTOP_USER_DATA_DIR/SESSION_DATA_DIR，避免跨步骤单实例锁导致 Node 模式不启动
  * tokio 1.53.1 -> 1.53.3（cargo-deny RUSTSEC-2026-0386）
  * 删除临时 Windows 诊断工作流

* **ci:** drill 使用独立 userData 隔离单实例锁 ([0bf5335](https://github.com/sekfung/ESCode/commit/0bf5335a204d16a540770f443bc9450b631f9153))
  * drill 设置 ESCODE_DESKTOP_USER_DATA_DIR/SESSION_DATA_DIR，避免与安装检查/上一次启动共用单实例锁
  * Windows 诊断工作流改用当前 release run 的 win-x64 安装包验证

* **ci:** Windows 暂存远程 Rust runtime 改用相对路径，修 tar 盘符误判 ([980bbb8](https://github.com/sekfung/ESCode/commit/980bbb87c4535cb0d65e5980f08e00f40460b7f2))
  * GNU tar 把首个斜杠之前的冒号当作远程主机（host:path），去连 rsh；
  * 同一路径里的反斜杠会让 bash 通配不展开，cp 的 * 会原样传给 cp。

* **cli-net:** 模型请求未解析出代理时直连，不再回落到进程 env 代理 ([24df385](https://github.com/sekfung/ESCode/commit/24df385e092a5e8537afc883e19e5df385bf3aa8))

* **cli:** finally 中抛错会顶掉迁移与启动的原始异常 ([0fdc747](https://github.com/sekfung/ESCode/commit/0fdc747a4abdb22a30f208663113c76cddf0b8f9))

* **deps:** 升级 rustls 与 zip 清除 cargo deny 安全公告 ([2eaa21d](https://github.com/sekfung/ESCode/commit/2eaa21d88c70b6a0b88d970c14a097c0cdbd80fa))
  * rustls 0.23.45：RUSTSEC-2026-0346/0347/0348/0349 → 0.23.46
  * zip 8.6.0：RUSTSEC-2026-0367/0368/0369 → 9.0.5（代码不调 extract()，

* **deps:** 升级 rustls 与 zip 清除 cargo deny 安全公告 ([6e13068](https://github.com/sekfung/ESCode/commit/6e130688451fe875692ccb25649d4e976836e331))
  * rustls 0.23.45：RUSTSEC-2026-0346/0347/0348/0349 → 0.23.46
  * zip 8.6.0：RUSTSEC-2026-0367/0368/0369 → 9.0.5（代码不调 extract()，

* **desktop:** 恢复安装包启动检查的数据库终态日志 ([2ac72d7](https://github.com/sekfung/ESCode/commit/2ac72d7925db8812206c689d9442211aa4faa123))

* **desktop:** Rust Host 启动失败按类别显式报错（H2.2 前半） ([8c8b67d](https://github.com/sekfung/ESCode/commit/8c8b67d687b8de06685d6c31cc7672d1cda4e639))
  * startRustHost 抛 RustHostStartError，code 分四类：BINARY_MISSING /
  * 修复：未监听子进程 error（二进制缺失时 Main 抛未处理异常、握手只能等满超时）；
  * manager 启动失败改记 error 并原样抛出分类错误，删除“回退 Node Host”的不实日志
  * 远控 bridge 把 RUST_HOST_* 映射到已本地化的 desktop-disconnected
  * 新增 rustHostResilience.test.ts 启动失败部分与映射/不回退用例

* **desktop:** Rust Host 桥接按 attachment 声明交付模式（修 A9 引入的手机 continuous 回归） ([adadf3a](https://github.com/sekfung/ESCode/commit/adadf3a17e7a870321d168eb70187ef2cb405c7c))

* **host-runtime:** 反向请求应答、实例退出即失败在飞请求、订阅溢出可观测 ([fd8a866](https://github.com/sekfung/ESCode/commit/fd8a8668ca22815acc1c7d3159918a1b657cb9b4))
  * A5 反向请求无人应答：帧泵把 runtime→Host 请求当普通帧扇出，runtime 等待
  * A1 实例退出时在飞 call 要等满超时：读端关闭即清空等待方（先置 closed 再清，
  * A2 订阅者 try_send 满即静默丢帧、关闭的订阅者从不摘除：满即摘除并置 lagged，
  * A8 child 链路 stderr piped 无人读，写满管道后子进程阻塞：逐行转发到 Host stderr。

* **host:** 拆分 escode-task 超长文件并修复 spec 格式 ([0082dd4](https://github.com/sekfung/ESCode/commit/0082dd40f0d453efd500c3fa57d29791a462445e))

* **host:** 队列口径与 spec 对齐，日志丢弃按写者计数（H1.4） ([7220678](https://github.com/sekfung/ESCode/commit/7220678d819af01c8e2d9ce0f8f971541a58d1e3))
  * 骨架 spec §3.2 逐队列写明常量与构造点（RPC 字节水位、runtime 扇出、
  * Host 日志取“队满时任何级别都可能丢，并计数”：sink 在同步上下文里调用、
  * 常量处注释引用 §3.2 表

* **host:** 结构化日志走控制队列（logBatch）；window-controller 监听任务加取消路径 ([fdc186a](https://github.com/sekfung/ESCode/commit/fdc186ac8f43cafc215ccf528616be2963c1eadd))
  * host-control::logging：进程级 sink（Arc<dyn Fn(LogBatch)>，同 CLI runtime file_log 形态），
  * ControlLoop：改为 ControlWriter 单队列（容量 1024）+ 一个泵任务串行写 stdout；队满时控制必达、
  * Main：startRustHost.onLogBatch 校验并消费 logBatch（不再进握手队列、不挤占 50 帧预算），
  * 38 处 eprintln! 改为 logging::warn/error，scope 取域名（window-controller / task-index /

* **host:** 评审跟进——controller 脏标记不丢、订阅/收口竞态、provider 文件指纹闸门 ([627b2ad](https://github.com/sekfung/ESCode/commit/627b2ad1382e73b7ff8c4f2ce12b5c3b60571eab))
  * window-controller：读库失败时保留“仍需刷新”标记（DirtyFlags：读库前清标记、失败放回），不再吞掉变更信号；
  * providers：ProviderService 用 FileStamp/ChangeGate（与 CLI registry 同机制）跳过未变文件的读盘与解析；

* **host:** 收口 M2 技术债与默认值差异 ([93ca4ed](https://github.com/sekfung/ESCode/commit/93ca4edb3dcddf450bcabaa3e660559ae64c0267))
  * host-rpc：handler 同步/异步 panic 捕获后回 Fatal 错误帧，请求不再悬空。
  * Main：prepareAppQuit 显式 stop Rust Host，不再只依赖 stdin EOF。
  * host-runtime：wait_ready 改 watch 通知；关闭立即返回 false，去掉 20ms 轮询。
  * settings：缺文件/坏文件回退 TS appSettingsSchema.parse({}) 默认对象；合法文件补齐缺省字段；真实客户端用 TS schema 做全等 oracle。
  * tasks-index：busy_timeout 25ms -> 5000ms（与 TS 长连接 TaskIndexRepo 同），补 PRAGMA 单测。
  * H1.8：CLI 二进制缺失时 child 用例失败并提示构建；instances 的 Windows 路径补 .exe；test:escode-host 先构建 CLI。
  * CI：新增 Windows rust-host-windows job 跑 rustHostResilience 的 errno 分类。

* **host:** 修复 escode-task 拆分后的编译错误 ([a1e92ec](https://github.com/sekfung/ESCode/commit/a1e92ec3f76ff33d31ff3bd6d8ed6650f6601a89))

* **host:** 修复 Windows 基础环境循环的 clippy auto-deref ([775074b](https://github.com/sekfung/ESCode/commit/775074bd7bca4dc42418c7d1a2f2f9162fbd8d5a))

* **host:** 修复 Windows host 测试 27 例；CI 手动触发可只跑指定平台 ([a155908](https://github.com/sekfung/ESCode/commit/a15590841beff005258df6ba1e90770bf4865ae3))
  * 产品缺陷：git checkpoint 的绝对路径以 PathBuf 逐段 push 到 git 返回的 `C:/...` 上，得到混合
  * 测试夹具：git_repo 在 Windows 上去掉 canonicalize 的 `\\?\` verbatim 前缀，与 Node realpath 一致
  * 测试拼路径：host 测试中 194 处 `.join("a/b")` 机械改为逐段 join（Unix 语义不变，Windows 得到原生
  * POSIX 写死的期望：gen_ui normalize/child_of、mcp_sync/provider 路径、file_index、storage 绝对路径
  * CI：workflow_dispatch 新增 platforms 输入，定位单平台问题时只跑该平台；push/PR 恒为三平台，

* **host:** 修正 clippy 告警 ([a513e48](https://github.com/sekfung/ESCode/commit/a513e4895f0011fac7fb481f9e6b6e30f04e2011))

* **host:** file_watcher 事件路径对齐调用方前缀；修复三平台 host 测试的平台差异与竞态 ([f26c933](https://github.com/sekfung/ESCode/commit/f26c933d40679070ecc062c8546c78cce51dbab4))
  * macOS：file_watcher 原样转发底层事件路径，FSEvents 报 /private/var/...，changedPath 与
  * ubuntu：host_io / wiki formats_office 在本进程写可执行脚本，并发用例 fork 继承写句柄致
  * Windows：node_join 期望值取自 POSIX path.join，Windows 上按 path.win32 分隔符断言
  * 全量本地复跑暴露：instances.rs 零期限超时依赖“应答不可能先到”的调度假设（内嵌 runtime 多线程

* **host:** git checkpoint 冲突路径同样按 Node resolve 语义拼接 ([d003e8d](https://github.com/sekfung/ESCode/commit/d003e8df7b727fd93fb2b11aeacaee695fd40e47))

* **host:** host-rpc 每连接登记序 = 到达序（H1.3） ([64d1bd1](https://github.com/sekfung/ESCode/commit/64d1bd125339a6f81d643d3c46789503fc42e073))
  * 新增 rpc_order.rs：单连接 64 请求登记序与倒序放行的响应关联、
  * 骨架 spec §3.4/§8：Gateway 到达序只适用于 host-surface，

* **host:** Main 合成的嵌入运行时 env 经 rpcListen.runtimeEnv 下发（审计 A16a） ([7f12af5](https://github.com/sekfung/ESCode/commit/7f12af5ec002fc1e55e73cbf28876ebd1f08a6c9))
  * host-control：RpcListen 增加可选 runtimeEnv（旧版 Main 不受影响）。
  * Host：runtime_options_with 以 Main 的映射为底，ESCODE_WORKSPACE_IDENTITY 由描述决定并优先。
  * Main：buildRustHostRuntimeEnv = Node Host 进程同源 env 去掉 OTLP 连接配置（不进会被工具继承的快照）；
  * 未修（A16b）：provider 配置文件路径与插件宿主入口，随 providers / 扩展域交付。
  * 验证：control_tests / host-control serde / 假 Host 握手测试 / desktopRuntimeEnv 测试，三处负向验证均失败。

* **host:** readdir 测试在 Windows 上的 needless_collect ([12b9bfa](https://github.com/sekfung/ESCode/commit/12b9bfa87f0d8eb103976dcd940bfc692fd3bf5a))

* **host:** runtime 实例崩溃隔离与即时回收（H2.7） ([818c040](https://github.com/sekfung/ESCode/commit/818c040fe3a6be577460e50aba1f2136cb653109))
  * 帧泵是唯一崩溃检测者：读端意外关闭后立即回收实例（等任务结束、
  * 在飞请求以类型化 RuntimeClosed 失败，文案逐字对齐 TS
  * Lifecycle::wait 幂等（JoinHandle 只 await 一次），并返回退出原因
  * embed 根任务用 catch_panic 包住，panic 时 abort_all 照常执行
  * escode-cli-core 新增仅测试的 fault-injection feature（test/panic）
  * 新增 crash_isolation.rs：embedded panic 与 child SIGKILL 两例

* **host:** Windows child runtime 补系统环境；补真实客户端崩溃错误用例 ([7da9bf0](https://github.com/sekfung/ESCode/commit/7da9bf0b4d6e2616801a9bd0e33fab4830a99336))
  * Link::Child 在 Windows 上 env_clear 后补回 SystemRoot 等系统底座：缺少
  * crash_isolation 同时等在飞请求/实例退出，不再把真实错误掩盖成 60s 超时。
  * 新增 rustHostRuntimeCrash.test.ts：真实 @escode/client + child runtime，
  * unit-tests CI 构建 Host/CLI 二进制，避免依赖真实 Host 的 desktop 用例被 skipIf。

* **installer:** 普通卸载保留安装目录内的 .escode 数据 ([dce85a8](https://github.com/sekfung/ESCode/commit/dce85a8da8659d9191cc2ba2864260d0e40d78df))
  * customRemoveFiles 新增 preserve-data 模式：普通卸载时递归命中安装目录树内的
  * 无 .escode 时保持原有整目录删除；覆盖更新路径不变
  * NSIS 夹具与 Windows 运行时脚本补充普通卸载两种场景；同步更新两份 spec

* **parity:** 归一 OS 版本并在 --golden 时重新归一冻结观察 ([548e265](https://github.com/sekfung/ESCode/commit/548e265faf8968751c85f1ff2bc14441f6b2d02f))
  * NORMALIZATION 增加 os-version（内核版本随 runner 变化）
  * golden 记录录制侧 root；--golden 加载后用当前规则重新归一 raw frames/modelRequests
  * 重录 linux-x64 session-baseline golden

* **release:** 发布门禁报告归档内路径统一用 `/` ([f9c6399](https://github.com/sekfung/ESCode/commit/f9c63993705e08db9cd6a1da5399b210f71d9336))

* **release:** release 产出正式版安装包，并加产物门禁 ([f8ffde0](https://github.com/sekfung/ESCode/commit/f8ffde0e97728d3068ae0307280e57d1bce8b85f))
  * release 工作流级设置 ESCODE_ENV=production，产出正式身份 ESCode / dev.escode.app
  * 新增 assert-release-artifacts.mjs，在打包后只读校验：生产环境且非 Preview、安装包文件名
  * 单测锁定 Rust release profile 的 strip = true，以及 release 工作流的生产环境与门禁步骤
  * 更新 release 说明中已过时的 Node 回退描述；新增 docs/desktop/release-artifact-hygiene.md
  * oxfmt 格式化 remote-rust-agent.mjs

* **runtime:** 嵌入实例根 future 先装箱，修复 catch_panic 包裹后的 worker 栈溢出 ([6ff53d7](https://github.com/sekfung/ESCode/commit/6ff53d7e60370ffda66558702b83f0030a41f583))

* **runtime:** 修复 Spawner 账目登记竞态 ([23b5112](https://github.com/sekfung/ESCode/commit/23b511230362c6154ddf540a276ff9497fcf2374))

* **runtime:** 修复 test:escode-cli-rust 的 5 个差分失败 ([42c80e0](https://github.com/sekfung/ESCode/commit/42c80e06fabbd857eab465af6b71c2edc3ac211e))
  * plugins-install：87f7aa44 错误类型化漏改 “Dependency not found” 产生点，
  * plugins-list：Node readdirSync 在 Unix 上经 libuv scandir 按字节序排序，
  * model-io ×2：MCP server 用 BTreeMap 合并导致按名字排序；新增
  * mcp-request-context：测试取第一个 mcp__ 工具，随包官方插件工具排在前面；
  * spec：rust-plugins.md 补技能顺序，rust-browser-use.md 补 MCP 合并顺序与

* **runtime:** 修复三平台 test 作业暴露的写入未 flush、gen-ui 通知丢失与 Windows 短路径崩溃 ([748e710](https://github.com/sekfung/ESCode/commit/748e710bf9bfc85307132dfecee5bbc12c8b0e4f))
  * tokio File 的 write_all 只把数据交给后台线程，未 flush 即返回时紧接着的读取可能读到空文件
  * gen-ui 状态 watcher 用平台回报的事件路径查 targets；macOS FSEvents 回报解析软链后的
  * Windows：fixture 的临时 root 为 8.3 短路径，runtime 拉起的 Node 子进程 fs.watch 触发 libuv

* **runtime:** 移除加速卡删除后的 turnHeader 计时字段 ([dbbabcd](https://github.com/sekfung/ESCode/commit/dbbabcdbe78d160b2f95bfca3b2c1333cd11e512))

* **rust-cli:** 按 ESCODE_HOST_CIRCUIT 注入内置 circuit MCP server ([02f54e9](https://github.com/sekfung/ESCode/commit/02f54e974b563c7b991ca78315f4c75445d9cbe5))
  * serial_broker 泛化为 host_broker，用 Spec 描述 serial / circuit 的方法表、
  * 新增 mcp_circuit：Host 标记 + 插件启用时注册，最后合并防劫持，
  * hub 为 circuit 单独启动 broker，连接材料只注入 circuit server env
  * 规格 docs/circuit/README.md §3.5 补充运行时注册规则

* **services:** 恢复 escode-cli-rust 测试套件的类型检查 ([10ab861](https://github.com/sekfung/ESCode/commit/10ab861068495a7584bcae6c68a9eb8a15db1c3d))

* **subagents:** 目录身份加入 birthtime，修复 ext4 inode 复用导致的监听失效 ([63acf75](https://github.com/sekfung/ESCode/commit/63acf75a8b73f809b4dc7f7e098be3d690d46258))

* **test:** 串口广播断言按平台、Windows 临时目录清理对 EBUSY 重试 ([b2c6d5e](https://github.com/sekfung/ESCode/commit/b2c6d5e699b0228dfda3c0b6740a016295ad80f8))
  * macOS host：rpc_serial 广播的 error.code 同样按平台断言（上一提交只改了应答处）
  * Windows：performance 用例收尾 rm 临时目录时 rust-sessions.sqlite 偶发 EBUSY；子进程已确定

* **test:** 归一 usage 日期、可选 rea 列表与 OS 家族 golden 回退 ([c7837b8](https://github.com/sekfung/ESCode/commit/c7837b8b4068616f6d77b172add154df974a2d27))
  * conversation-usage 归一 startDate/endDate/date
  * plugins-uninstall 过滤可选 rea；删除对应平台 golden，回退通用观察
  * nodeOracle 在精确 arch 未命中时回退同 OS 的任意平台 golden
  * Linux test 作业安装 poppler-utils，匹配 PDF golden 的录制环境

* **test:** 归一 workspace 路径与 installed 文件反斜杠 ([63dee1c](https://github.com/sekfung/ESCode/commit/63dee1c3e2d1127651327b9dd28bc30e1d392243))
  * canonical 对 workspace/workspaceKey/directory/cwd/root 也统一正斜杠
  * migration-live 的 JSON 列即使命中 raw 占位符也写入 JSON 转义路径（Windows invalid escape）
  * plugins-uninstall 的磁盘原文把双反斜杠统一为正斜杠

* **test:** 归一路径分隔符并修正 Windows seed workspace 替换 ([282b51b](https://github.com/sekfung/ESCode/commit/282b51b05ff60232ff458f9601ae1867594af5ce))

* **test:** 稳定 CLI golden 的 CPU/本地化/可选插件差异 ([5c2e404](https://github.com/sekfung/ESCode/commit/5c2e404ef126dc356f1f55adcdd76f07c2013922))
  * canonical 归一 concurrencyCeiling/ceiling/cap、内核版本与 bash 本地化错误
  * plugins-list 过滤可选 rea 插件，避免随包 bundle 差异
  * 生成器写入/检查移动到 crates/plugins 的 plugin_defaults.json 与 official_plugin_sort.json

* **test:** 修复 macOS/Windows test 与 bench 作业暴露的平台差异 ([4f1f466](https://github.com/sekfung/ESCode/commit/4f1f46649f4839b57b569d248ac63f51410423fe))
  * wiki storage_layout：项目根经 git 解析为真实路径，macOS /var→/private/var、Windows 8.3 短名
  * file_stamp：同长度原子替换的指纹断言限 Unix。Windows 无 inode/ctime，按模块设计由
  * plugin_skill_files：测试路径分隔符统一为 `/` 再比较
  * bench：Windows tmpdir() 为 8.3 短路径，Node CLI 的 fs.watch 回报长路径触发 libuv 断言

* **test:** git 夹具固定 core.autocrlf=false；file_watcher 根自身事件单测按平台分隔符 ([ed40d84](https://github.com/sekfung/ESCode/commit/ed40d845e7fc107ccd85a66e29346e41430ad2e2))
  * rpc_git_checkpoint：GitHub Windows runner 系统级 core.autocrlf=true，git restore 写回 CRLF，
  * events::root_itself_maps_like_node_inotify：上一提交把根自身事件改为平台分隔符时漏改本单测期望

* **test:** nodeOracle 实际加载平台专属 golden ([961c661](https://github.com/sekfung/ESCode/commit/961c661f4c35dbc1313c20eea1e7b7ea4a9b6f7b))

* **test:** Path 字段先归一环境；migration-live 回落共享 seed golden ([80639d2](https://github.com/sekfung/ESCode/commit/80639d22901a2fc3b98d3d552da0cb01f34ac78f))
  * /Path$/ 分支先跑 canonical（日期/临时目录），再统一为 /，避免 macOS /var/folders 泄漏到断言
  * migration-live 的平台 golden 含各自 Node 会话 id，与共享 seed DB 不匹配；删除后回退 generic

* **test:** rpc_gen_ui 逐段拼路径；test:escode-host 不在首个失败处停止 ([7add37b](https://github.com/sekfung/ESCode/commit/7add37b8774b7fa92b223984939ef211fcf963a2))

* **test:** runtime 退出窗口的偏好请求应答不再带崩 fixture ([b3279c4](https://github.com/sekfung/ESCode/commit/b3279c46185aaff61f32f96469f393c083a81ba3))

* **test:** Windows 临时目录统一长路径，修复差分归一化与 parity 挂起；串口用例按平台断言 ([7ebdc65](https://github.com/sekfung/ESCode/commit/7ebdc65c4a8f0e5daf94c98ef27aafd31d864f07))
  * Windows test 剩 25 例差分失败：上一提交只在 fixture 内对 root 做 realpath，用例自持的短名
  * Windows bench：bench 步骤已通过，parity 场景在短名 capRoot 上子进程崩溃后空等到作业超时；
  * macOS host：rpc_serial 的 open 失败码在非 Linux 为 nativeUnavailable（spec §9 登记差异），

* **third-party:** 修复声明流水线并重新生成清单 ([ab938ab](https://github.com/sekfung/ESCode/commit/ab938ab3979ffb0bf22d713ae955f135a37c6af4))
  * 锁文件一侧改为直接解析 pnpm-lock.yaml（scripts/third-party-lockfile-graph.mjs，也是 README
  * 安装树一侧改为仓库既有的目录递归扫描：hoisted 布局下 pnpm 的报告会漏掉嵌套副本
  * 交叉校验：以流水线健康时产出的 inventory（1219 个生产包）为基准，40,230 条边解析、92 条未命中
  * THIRD-PARTY-NOTICES.md 与 inventory.json（1178 包 / 13 copied 组件 / 18 原生归档）。
  * native 配置哈希按 README「原生搜索」口径先核对组件与版本未变再刷新（单行 diff）；
  * @electron-internal/extract-zip@1.0.5 随 electron 升级进入生产闭包且发布版缺完整许可
  * keyv@4.5.4 的 override 随该链移出生产闭包而过期 → 删条目与其孤立的上游原文快照。

* **ui:** 错误边界生产构建落盘崩溃根因 ([1a4ac67](https://github.com/sekfung/ESCode/commit/1a4ac672736cf919404a0e02fdc16d043d3c0b3d))
  * 根因/级边界改走 logger.lifecycle.error：renderer 生产构建下普通 logger.error
  * componentStack 在 React 19.2 生产构建里由 fiber 链合成，每帧只指向组件定义，
  * 补充生产构建下单测，验证经 window.escode.log 桌面桥落盘且不写 console

* **ui:** CircuitPanel 的 labels 内联对象破坏 CircuitSchematicView 的 memo ([a409072](https://github.com/sekfung/ESCode/commit/a4090723035ce040a011a511b6b10dda629e81a4))

* **wiki:** refuse re-promoting over an existing global copy; spec import entry ([712df44](https://github.com/sekfung/ESCode/commit/712df44eeda74ed367265fee846b5015066503f5))
  * promote: a second promote of the same project entry overwrote the existing
  * ui: useWikiKnowledge exposes a single manage(request) write path
  * spec §11: proposed Desktop import dialog and scope rules (pending approval)


### Chores

* 补充 automation cron 依赖并清理分享残留文案 ([4b5a095](https://github.com/sekfung/ESCode/commit/4b5a0958862f679296343cdc3e79c13df95a7a93))

* 清理套餐族删除后的残留引用与死代码 ([e3abafa](https://github.com/sekfung/ESCode/commit/e3abafa75382ef218d2f35bb686202691f117dd9))
  * desktop：修复 tsup 仍以已删 preload/codingPlanWebview.ts 为入口的构建错误；
  * shared：删 CodingPlanWebviewChannels 与购买/locale 类型、isTrustedCodingPlanWebviewOrigin、
  * E2E：删除指向已删 UI 的 7 个 spec 与 4 个 helper、6 个 highspeed 夹具，以及 OAuth 登录
  * 测试同步：desktopWindowChrome / desktopMainIpcRemoteTelemetry / e2ecase-regression /

* 清理问题上报（feedback）删除后的残留引用 ([cb24cd8](https://github.com/sekfung/ESCode/commit/cb24cd82a2281ea9715250c62ad0e958023ef4df))
  * artifactPresentation 注释不再引用已删除的 feedbackSubmissionJob
  * 能力图去掉「反馈工单 / 反馈附件」描述；审计脚本去掉 feedback 服务目录
  * 内存审计记录标注该模块已随功能删除；E2E 错误横幅动作标签去掉「提交反馈」

* **circuit:** 移除电路分析插件与技能，整条链路下线 ([fb906df](https://github.com/sekfung/ESCode/commit/fb906dff36bb63dc0a24503fc3929c302b142221))
  * 插件与技能：删除 apps/escode-cli/packages/circuit-plugin（MCP server 与 circuit skill）
  * bootstrap：删除 built-in-circuit / circuit-broker / circuit-control-broker，并从官方插件定义
  * Rust：删除 mcp_circuit.rs 与 host_broker 的 CIRCUIT spec，整理 mcp_hub、mcp_hub_host_tools、
  * shared / services：删除 Circuit IR 契约与 CircuitService（KiCad / EasyEDA 解析、netlist、BOM、
  * ui / client：删除电路面板与原理图渲染、引用卡片与 chip、useCircuitPanel、预览面板路由、i18n
  * docs 与构建：删除 docs/circuit 规格、ADR-0005、e2e 用例与 .feature、architecture-policy 模块、
  * CHANGELOG 与历史计划归档保持原样，不改写已发布记录

* **deps:** electron 41.0.3 → 41.10.7 ([114b75a](https://github.com/sekfung/ESCode/commit/114b75a2b53c363e36106eb0a840f840767906ea))

* **fmt:** 排除 vendored 的 docs/electron/docs 免于 oxfmt 解析 ([ccdc42d](https://github.com/sekfung/ESCode/commit/ccdc42d97a3e06980c914d51897f7197e6857bb0))

* **host:** 限制测试目标的 debuginfo ([2a18ec7](https://github.com/sekfung/ESCode/commit/2a18ec7f64f92fe9263501a0bd289e5ef667a985))

* **lint:** 修正 bootstrap 包级 lint 的 max-lines 豁免 ([7795a24](https://github.com/sekfung/ESCode/commit/7795a242705563a18daad248c8cb6d2d7ced7262))

* **runtime:** Node 统一为 24.18.0（与 Electron 自带 Node 一致） ([de65889](https://github.com/sekfung/ESCode/commit/de658898f40f5fe8dab153880e7b523c4b92f315))
  * mise.toml、apps/escode-cli/.node-version、apps/escode-cli/package.json engines → 24.18.0；
  * docs/development-mise-runtime.md 的两处现行口径同步（历史验证记录不动）。

* **vscode:** 指定 native-search-tools 的 CMake 源目录 ([ce726b6](https://github.com/sekfung/ESCode/commit/ce726b64ad61a681b76c0df82349d596c3ef505f))


### Documentation

* 记录 CLI 平台 golden 与跨平台 CI 结果 ([ff74024](https://github.com/sekfung/ESCode/commit/ff7402411510bc05933451795d8d81d9c8f35bef))

* 记录 installed 四平台 CI 证据并勾选阶段3 ([7dc7d36](https://github.com/sekfung/ESCode/commit/7dc7d36816c1c07f85a2035d3c0df9399deb720b))

* 记录默认翻转后的四平台安装包验收 ([93541ee](https://github.com/sekfung/ESCode/commit/93541eedde5ca487f3565bf906885d05a6cdd7c0))

* **host:** 补记 subagents.list 的逐项读码结果与两处待定口径 ([374dc66](https://github.com/sekfung/ESCode/commit/374dc66030f5d33b81c0c7511862f8bf0ae6f81f))
  * Settings 侧用的是真正的 yaml 包（YAML 结果被宽松解析覆盖，只有 mcpServers 数组以
  * 插件发现用的是另一个缓存扫描器 officialPluginCache.ts（按插件名分组、版本目录数字
  * 补记内置 agent 常量、文件发现的「只扫第一层」、插件命名/别名/歧义诊断、

* **host:** 登记 serial 作用域决策（每连接≈每窗口） ([94263ef](https://github.com/sekfung/ESCode/commit/94263ef926aeb95194c760a0040220952db5c34a))

* **host:** 库存复核 + Host 范围边界登记 ([af2a3fe](https://github.com/sekfung/ESCode/commit/af2a3fed670333afa7a774633ca60a5b5132c461))
  * serial(14) 可迁：零 Electron 引用，阻塞是 @serialport/bindings-cpp 这个
  * cua-permission(2) 是**缺口**不是非缺口：它在 Renderer 可达面上

* **host:** providers 域设计盘点——47 个成员、P1–P6 切片、3 个待决策项 ([0e5172f](https://github.com/sekfung/ESCode/commit/0e5172f9d8fedce008b66e47864368a27c96b650))

* **rea:** 验收记录按交接更新（受影响单测/bootstrap 套件交给 CI，打包核对交给打包） ([77ec3b0](https://github.com/sekfung/ESCode/commit/77ec3b0b634415a95b9104c41e7bbbe26a704328))
  * 受影响单测与 bootstrap 套件 → unit-tests workflow（affected + bootstrap 两个 job）；
  * 打包产物核对 → 一次真实打包；
  * 真实桌面应用与 Rust 端到端链路、macOS/Linux → CI 矩阵；
  * 20 项待核验材料 → 发布前 licenses check --strict。

* **report:** 补全 M3 hooks 提交列表 ([ce75ae5](https://github.com/sekfung/ESCode/commit/ce75ae5a3624ae017e62ae7fccbe2a43252e5fde))

* **report:** 登记七项修复的 Windows 与本地验证结果 ([6007bfc](https://github.com/sekfung/ESCode/commit/6007bfcf52b0859fee1fdf74166b6928f22d941a))

* **report:** 复核并登记 M2 未完成项与证据 ([5e1e940](https://github.com/sekfung/ESCode/commit/5e1e9406c6b79038f58c3922db275e0c3ed4c711))

* **report:** 记录 M3 全量状态与剩余阻塞 ([0c255f3](https://github.com/sekfung/ESCode/commit/0c255f3d41fd6d4323c300669d8fce711cd45b74))
  * plugin-sync/零散成员已完成项与证据
  * off-peak/automation、bots、conversation-share、marketing-touch 仍未完成
  * 明确 M3 当前不可验收

* **report:** 记录 M3 hooks 的完整 CLI 套件结果 ([14d3910](https://github.com/sekfung/ESCode/commit/14d391058fa20f72aaa531676b85d2c48c7ca242))

* **report:** 记录 unit-tests affected/bootstrap 收口 ([dfebb3a](https://github.com/sekfung/ESCode/commit/dfebb3a2e2774f90505095fdfd26d917f9b9474c))

* **report:** 记录 Windows crash isolation 根因、真实客户端用例与三平台结果 ([93c4f2b](https://github.com/sekfung/ESCode/commit/93c4f2b8426980f0afc07240541c1b90f637048a))

* **report:** 三平台 rust CI 全绿；登记 Windows errno 未验证项 ([632c1fb](https://github.com/sekfung/ESCode/commit/632c1fb427c8f01650eaaecb6e22fa059efa2b8f))

* **report:** 校正 M3 状态 378/161 与已完成项 ([91f8428](https://github.com/sekfung/ESCode/commit/91f8428213202b9753ef79a41ba5f83fb7148427))

* **report:** M1 H0 三平台 CI 关闭，补 run 39164847925 证据 ([91fbbe0](https://github.com/sekfung/ESCode/commit/91fbbe07ee64c950d43c31674e4764b9254e579a))

* **report:** M3 分支改名 m3/milestone ([1db268d](https://github.com/sekfung/ESCode/commit/1db268d26acee7c465b9078a504e0f636d782780))

* **reports:** 新增 Rust 迁移 M1 报告 ([4463e50](https://github.com/sekfung/ESCode/commit/4463e5024484270439d4aed8f11e389e8d73d720))

* **reports:** 新增 Rust 迁移 M2 报告并订正骨架 spec 回退表述 ([8446a11](https://github.com/sekfung/ESCode/commit/8446a11f83bdb2c46cc308e9ed2a76bd19cf3296))
  * M2 报告：T0 bisect、H2.7/H2.2 前半/Host 崩溃/H1.3/H1.4 逐项证据、负向验证与
  * 骨架 spec dev 开关段：原“失败抛错由调用方回退 Node Host”与代码不符，改为
  * 验收报告“后续”行追加 M2 链接（正文不改）

* **reports:** 新增 Rust 迁移独立验收报告（2026-10-18） ([17f616f](https://github.com/sekfung/ESCode/commit/17f616fb7838fa5dae3884d449b082374f3b2796))

* **reports:** M1 复核收口，R 线 ACCEPT ([0cc6009](https://github.com/sekfung/ESCode/commit/0cc600962e7767df9ae32b56af352943c96a0445))
  * 推翻“5 个既存/环境敏感失败”的判定：逐项记录真实原因与修复（42c80e06）
  * cargo deny 本机运行：rustls/zip 安全公告修复后四项全 ok（6e130688）
  * R9 PARTIAL → PASS；R 线 ACCEPT WITH CONDITIONS → ACCEPT
  * 如实登记验证覆盖范围：同期 H2 提交（818c040f 等）不在本轮套件覆盖内

* **reports:** M1 复核收口，R 线 ACCEPT ([f699665](https://github.com/sekfung/ESCode/commit/f6996652941b9b9e2bb4eec24367d7040128a387))
  * 推翻“5 个既存/环境敏感失败”的判定：逐项记录真实原因与修复（42c80e06）
  * cargo deny 本机运行：rustls/zip 安全公告修复后四项全 ok（6e130688）
  * R9 PARTIAL → PASS；R 线 ACCEPT WITH CONDITIONS → ACCEPT
  * 如实登记验证覆盖范围：同期 H2 提交（818c040f 等）不在本轮套件覆盖内

* **reports:** M1 最终状态与分支说明 ([4e10dfb](https://github.com/sekfung/ESCode/commit/4e10dfb0bbc6b42f596959338e4092295ba447af))
  * Host deny.toml、checks 三平台矩阵已补；H0「CI 三平台」待推送取得运行记录（UNVERIFIED）
  * 记录 M1 提交从 m2/host-resilience cherry-pick 回本分支，本分支代码与第 2 轮完整套件被测代码一致
  * m1 分支末端复核：边界、fmt、clippy、cargo deny（CLI/Host）全部 exit 0
  * 登记 m2 分支 Host 400 行超限归属 H2 工作流

* Rust 统一 Host 迁移交接手册（供其他会话接手） ([3477996](https://github.com/sekfung/ESCode/commit/347799615846bd26109ac3a730f9c4821aeadc55))
  * 五分钟上手：基线自检、必读文档顺序、门禁三连、**输出当前 channel/method 缺口的脚本**；
  * 代码地图：Host 各 crate 职责、TS 语义真相源（`escodeTaskServiceAdapter`/`taskIndexRepo`）、
  * 已落地清单（按域，含证据文件）与"未推送 36 个提交、无 PR"的事实；
  * **固定动作**：spec 先行、一片一提交、验收必须对照 TS（差分 > 端到端 > 单测）、
  * **加一个服务方法的固定形状**（四处改动点 + handler 模板 + 自检顺序）；
  * **陷阱清单**：共享工作树禁 `git add -A`、cargo 与差分套件不能并发、vitest 用的是已编译二进制
  * **已登记差异 6 条**（接手不要丢、也不要私自"修好"）；
  * **下一步逐项**：`getTaskSnapshot`（含 TS 函数行号锚点与快照键实测形状）、`getTaskConfigOptions`、
  * 名词约定速查（方法表即分派、实例/身份键、embedded vs child、两条链路语义、Host 不读进程 env）。

* **rust:** 记录 P1[#6](https://github.com/sekfung/ESCode/issues/6) 后的差分复跑归因与共享工作树的暂存纪律 ([abfd718](https://github.com/sekfung/ESCode/commit/abfd7187aff1bb8e45d2260bd098d8dd16648bfe))
  * 11 例失败逐个归因：4 例用阶段 0 基线二进制复现（本机既存）、5 例插件类为并发工作流
  * 运行纪律新增：禁止 git add -A（共享树含他线未提交改动），按路径精确暂存

* **rust:** 记录阶段 1 增量与差分失败分类（本机既存 2 例 + 浮变 2 例） ([38794c3](https://github.com/sekfung/ESCode/commit/38794c3c73379a6e5b241ba6e10476114f5b5f77)), closes [10/#11]()
  * 验收报告 §5 补：schema 增量复跑 393/2/12；两个既存失败（legacy-session、history 损坏边界）

* **specs:** 新增 Host 韧性子 spec（实例崩溃、Host 重启、启动失败、保序与队列） ([4f0de02](https://github.com/sekfung/ESCode/commit/4f0de02c0bcd7769b653e38e19207f7328210dfe))

* **wiki:** record final verification results in delivery report ([586e501](https://github.com/sekfung/ESCode/commit/586e501984cc975a212a039c9ac361128dfa449e))


### Refactorings

* 清理会话分享残留代码 ([f3275c9](https://github.com/sekfung/ESCode/commit/f3275c972c50402b480f6881b74ea5b53ba7957f))
  * 删除分享导入专用的草稿初始化标记与 seedImportedSessionDraft\n- 移除 ConversationTimeline headerSlot/分享选择相关测试与死代码\n- 清理源码/文档中的分享注释，旧 shared_context 枚举仅保留读取兼容

* 清理手机远控残留死代码与文档 ([1f670e9](https://github.com/sekfung/ESCode/commit/1f670e9d25a481e54d36e04d51da91092b0f2af1))

* 删除 Bots 聊天平台集成并登记为非迁移缺口 ([ed6ef80](https://github.com/sekfung/ESCode/commit/ed6ef8061336fb5cce783c9d6d752bebdfb140df))
  * 移除 services/shared/UI/Desktop/Server 的 bots 服务、群协作、协议与接线
  * 移除 Rust Host bots 域与 task bot 成员，重算 inventory 为 41 channels / 473 members / 349 Implemented / 124 Pending
  * 移除 CLI/CLI-Rust 的 ReplyToChannel、ReadSessionContext.topic、automation bot delivery 与生成语料
  * 删除 bots 专属文档、测试、E2E/fixtures，并在 M3 报告与 scope boundary 登记
  * 保留 bot_topic_context 只读投影与 bot_delivery_target 冻结列兼容

* 删除 off-peak 闲时任务并登记为非迁移缺口 ([3b4c5ad](https://github.com/sekfung/ESCode/commit/3b4c5ad3a0f7959596e388b370ee6acb00f56f7d))
  * 移除 off-peak 的 services、UI、调度器、TS/Rust CLI 工具与 Rust Host 服务/RPC
  * 协议删除 offPeak* 字段与 offPeak/create、offPeak/list，清理任务表与迁移
  * 新增 0004_drop_off_peak 迁移清理旧库 off_peak_tasks 与投影列
  * M3 清单重算为 43 channels / 522 members，off-peak 不再计入 Pending

* 删除会话分享并登记为非迁移缺口 ([2c64740](https://github.com/sekfung/ESCode/commit/2c64740dfd2774da0e853c50d05f76725b189d4f))
  * 移除 web-share/share-ui 落地页、services 发布/导入、shared 协议与 UI/Desktop Deep Link\n- 清理 TS/Rust CLI shared_context 导入与持久化，保留 claudeCode 导入\n- M3 清单重算为 42 channels / 513 members / 144 Pending，feature graph 与 inventory 同步

* 删除手机远控并登记为非迁移缺口 ([1df037e](https://github.com/sekfung/ESCode/commit/1df037e6df1cbc4d132ea082f0e44858f4830c72))

* 删除问题上报（feedback）功能，并移出 Host 迁移清单 ([961822d](https://github.com/sekfung/ESCode/commit/961822d11ab0883356685d24622bbd84a364bfdf))
  * services/shared/ui/desktop/web：移除 feedback 服务面（create/list/tickets/附件/日志归档）、
  * i18n：中英各删除 300 个 feedback.* 及相关键，聊天错误横幅复制文案改用 chat.error.copyText.*
  * Host 迁移清单：重新生成 inventory_ts.rs，feedback channel 不再登记
  * 文档/技能/能力图：删除 16 篇专项文档并清理残留引用

* 删除遥测上报并登记为非迁移缺口 ([2b93607](https://github.com/sekfung/ESCode/commit/2b9360711f012566cf93b4cea7deba362b212782))
  * 移除 /event/report、ARMS RUM、CLI OTLP、TTFT 导出与用户操作 Trace
  * 清理构建期端点、依赖、IPC/协议成员、测试、文档与 feature graph
  * 保留 deviceMid、Resource Manager、本地内存/崩溃诊断与 usage 统计
  * M3 inventory 重算为 41 channels / 468 members，遥测不再计入 Pending

* 删除营销服务与云内容链路 ([78d487f](https://github.com/sekfung/ESCode/commit/78d487f0eacbd7b73efb937a7de34119df147c8b))
  * 移除 services/shared/ui 的 marketing-touch、cloud-content 实现、单测与 E2E fixture
  * 清理设置页导航、i18n、样式、构建配置和 Host 生成清单
  * 更新 M3 报告与 host 边界文档，营销服务不再计入迁移缺口

* 下线 Coding Plan / 加速卡 / BigModel 套餐与账号登录相关功能 ([79ca84a](https://github.com/sekfung/ESCode/commit/79ca84a673ac067a0b7bb8149a284467cdc39dc7))
  * packages/services、packages/shared：移除 coding-plan-subscription、highspeed、
  * packages/ui：移除登录与 WelcomeScreen、套餐/额度/购买入口、加速卡行与分享、
  * apps/escode-cli：移除 OAuth/project token、endpoint-routing、bootstrap 登录流程等
  * apps/escode-host：内置 provider 源（builtin_source）与宿主侧配套调整
  * 协议/菜单/埋点/i18n/单测/文档与能力图同步清理

* **circuit:** ::circuit 在非 Desktop 不渲染卡片 ([666e193](https://github.com/sekfung/ESCode/commit/666e193994415053a46fec834cdc1cc74bd7c06a))
  * CircuitReferenceCard 在非 Desktop 直接返回 null，删掉静态分支与 MonitorIcon。
  * circuitReferenceSchema 去掉只为降级卡片存在的 componentCount / netCount（Desktop 面板读 IR 自己算）。
  * 同步删 i18n circuit.card.componentCount/netCount/desktopOnly，更新单测、E2E 场景 7、

* **circuit:** 删除内置规则与模型识别路径 ([7c12d64](https://github.com/sekfung/ESCode/commit/7c12d64e386f08a43aa97b4a3828e26ae32ae35f))
  * 删除 circuit_check 与六条内置规则：KiCad 自带 ERC 用符号里的真实电气类型，本实现只能用
  * 删除 circuit_submit_recognition、图片/PDF 识别、矢量 PDF 证据提取与识别评测脚本。
  * 保留 confidence 字段与面板低置信度标记：Agent 修正新建的元素仍记 0.5。
  * 保留电路面板、Circuit IR、circuit_parse/query/correct/export 与固件比对。
  * 工具数 8 → 6，免批准名单 7 → 5；转发固件比对依赖的网名谓词到 domain/netNameFacts.ts。
  * services 不再依赖 pdfjs-dist（连同本地 ambient 声明一起移除）。

* **host:** 按有效代码行拆分 automation 模块 ([a28d767](https://github.com/sekfung/ESCode/commit/a28d767f0189df0f414c2a05c9c93125b3cfb7f7))

* **host:** 拆分 M2 超长文件守住 400 行边界 ([fa109b7](https://github.com/sekfung/ESCode/commit/fa109b707dc264b82a9bf17488b643d1e1291ef7))

* **runtime:** 拆分 scope 测试守住 400 行 ([86ea79d](https://github.com/sekfung/ESCode/commit/86ea79d4ad406947fe68103bb9c7508bdf1d6b38))

* **runtime:** 将剩余裸 spawn 收口到 Spawner ([f5bfc46](https://github.com/sekfung/ESCode/commit/f5bfc4635a50fed33b66a6fbce9ae38dd81a79bc))

* **rust:** 插件错误类型化（P1 [#9](https://github.com/sekfung/ESCode/issues/9) 子项） ([87f7aa4](https://github.com/sekfung/ESCode/commit/87f7aa44a204a92c594f225b16ec46a26271546f))
  * 新增 crates/tools/src/plugin_failure.rs：Kind 枚举 ↔ TS 诊断 code（plugin_not_found /
  * 产生点改抛类型化失败：plugin_install_cache（Plugin not found）、plugin_install_entry
  * 两处重复的文案前缀匹配收敛：plugin_install::install_diagnostic 与
  * 测试：分类单点单测（code 映射、context 包裹后仍可分类、兜底）；

* **rust:** SKILL.md 扫描合并单点（P1 [#9](https://github.com/sekfung/ESCode/issues/9) 子项） ([143f0c3](https://github.com/sekfung/ESCode/commit/143f0c3ca09b372f47fd3789610d226f8f384593))
  * 新增 crates/tools/src/plugin_skill_files.rs：逐字对齐 TS
  * 三处调用收口：plugin_list_components::skill_files_under / count_skill_files（跨根去重）、
  * 测试：共享扫描 2 例（两级扫描与过滤、符号链接根与链接文件拒绝）；


### Other Changes

* docs/refactor(host): 评审 window-controller 交付——更正文档事实并清理小问题 ([6271f45](https://github.com/sekfung/ESCode/commit/6271f45085960abe27c0252e07ae6acee8fa9ffd))
  * 交接手册“escode-task 写面 Renderer 零调用”只对九个 prompt 生命周期方法成立；分组视图写面五法、Claude 会话迁移两法、
  * providers P2 不解锁 A16b（需 P5 Built-in 物化），也不单独解锁 git.generateCommitMessage。
  * window-controller spec 的 useGlobalTaskList 调用点数更正为 10 处/8 个源文件。

## [3.16.0](https://github.com/sekfung/ESCode/compare/v3.14.0-rust-rc.1...v3.16.0) (2026-10-12)

### Features

- 桌面端默认 runtime 切换为 Rust ([7950284](https://github.com/sekfung/ESCode/commit/7950284d339835c0663c4daf14713e78fbfe13b3))

- **circuit:** 电路面板、引用卡片与入口（G5） ([58954ab](https://github.com/sekfung/ESCode/commit/58954abd436ae53ab948e74d8c3301d710f2ac66))
  - hooks：useCircuitPanel（服务访问、按 (revision, stale) 比对刷新、stale/orphaned/写冲突状态）、useCircuitDraftReferences
  - 面板：layout 自绘 SVG（shapes/wires/junctions/labels/pins/texts，多图纸切换）、点选器件/网络高亮、
  - 入口：文件树 .kicad_sch/.kicad_pcb/.circuit.json 路由；::circuit 卡片（Gen UI 解析规则）打开面板定位 IR；
  - 只读查询扩展：ref/net/block 过滤全部省略时列出该类全部（BOM/网络表要枚举事实来源），id 与名字同时给出仍拒绝
  - 平台能力位新增 supportsCircuit（Desktop true，Web/手机 undefined），与 supportsSerialPort 同构
  - 测试：ui 4 文件 56 个；电路相关回归 18 文件 193 个
  - E2E 用例 docs/test-cases/e2e/circuit-panel.feature（7 个场景）；wdio 实跑受环境阻塞，详见交接说明

- **circuit:** 接入 circuit_check 与 circuit_compare_firmware 工具（G4 收尾） ([08cecb3](https://github.com/sekfung/ESCode/commit/08cecb3b68bed774724ce3bf61469be0bf1497e0))
  - 7 个 op 全部打通（parse/get/query/correct/check/compareFirmware/export）；submit_recognition 仍留给 G6
  - shared：broker 信封与 interaction/circuitCheck、interaction/circuitCompareFirmware 反向方法；协议方法表补两项
  - compare_firmware 请求补可选 cursor（此前 nextCursor 是死字段）
  - bridge/router：check/compareFirmware → 服务方法，cursor/files 透传，错误码沿用既有映射
  - 插件：两个只读工具注册；skill 补「规则结果与模型建议分开」「固件比对用于联调」，
  - 审批名单：check/compareFirmware 为只读放行，export 仍需批准
  - 测试：services+shared 15 文件 136 个、circuit-plugin 12 个、bootstrap broker 11 个

- **circuit:** 内置规则检查与固件比对（G3） ([5bc2687](https://github.com/sekfung/ESCode/commit/5bc2687014c9936f0d6aeebd9f9cd80c8ccc75ce))
  - 6 条内置规则（unconnected-pin / single-pin-net / undriven-power / i2c-missing-pullup /
  - .escode/circuit-rules.json 只读配置：启用/停用与级别覆盖；不合法整份忽略并报告 invalid
  - 固件比对：devicetree（.dts/.dtsi/.overlay）、STM32CubeMX .ioc、C 头文件引脚宏，四类差异带来源行号
  - 规格补 §6.1 配置格式 / §6.2 判定口径与 P1 边界 / §7.1 固件解析范围（IR 无电气类型与坐标，用确定性代理）
  - fixtures：每条规则一对 hit/miss 最小原理图 + 手写固件样例（埋 4 处不一致）
  - 测试：规则 12 + 配置 16 + 固件 15 + 契约 6（circuit 模块合计 116 通过）

- **circuit:** 识别提交与离线评测（G6） ([d2d6198](https://github.com/sekfung/ESCode/commit/d2d619873690390dae21f15e130d8b51b21be93b))
  - circuit_submit_recognition：服务端 schema + 一致性校验，最终元素 id 由服务端生成并重映射草稿内引用
  - 置信度只由服务端给出：基准 0.9 按校验扣分、夹在 [0.05, 0.9]、永不等于 1；矢量 PDF 文本证据命中记 1
  - 矢量 PDF 优先直接提取文本与线段（pdfjs-dist：CTM 跟踪、q/Q/cm、左上角原点，与面板叠框坐标一致）
  - 图片/PDF 按内容哈希派生 irId：内容一变即新 IR（新 id、revision 从 1、旧修正不迁移）
  - 同内容重新提交沿用 parse 前置条件（缺 baseRevision → invalid_ir，落后 → stale_revision），为此 schema 增可选 baseRevision
  - 浮点四舍五入到 3 位，避免浮点尾数写入 IR
  - 工具链路：broker/桥接/插件/skill 全部接入（免批准）；skill 补「先告知用户再发图 / 矢量 PDF 优先 / 低置信度必须说明」
  - 离线评测脚本 scripts/circuit-recognition-eval.mjs（本机 kicad-cli 导出语料，缺失即报错并给安装/手工放置说明）
  - 度量纯函数与单测（不进 CI 门禁）；报告写明模型标签、样本数、匹配口径与未匹配清单
  - 测试：services circuit 16 文件 149 个、整链路 8 个、plugin 13 个、shared/metrics 40 个

- **circuit:** broker、内置插件与远程访问（G4） ([257fd76](https://github.com/sekfung/ESCode/commit/257fd76cd461c9401ea0d5828d11e2b0bede7d82))
  - Host 侧：circuitAgentBridge（工具→服务方法、错误码映射）、circuitAgentRequestRouter（interaction/circuit\* 严格校验与响应复核）
  - 工具：circuit_parse/get/query/correct/export；export 走权限审批（不在 defaultAllowedTools）
  - 服务契约新增 exportSnapshot（快照为派生产物，服务不读回，§2 规则 8）
  - CLI 侧：进程级 circuit broker、bootstrap 反向控制通道、circuit-plugin（MCP server + circuit skill）注册为官方插件
  - shared：circuit broker 信封与反向方法、escode-protocol 方法表；Rust official_plugins.json 重新生成
  - 远程访问：remoteServiceAccess 接入 CircuitService，贯穿 workspaceIdentity/remoteSessionId
  - 未注册 circuit_submit_recognition（G6）与 circuit_check / circuit_compare_firmware（等 G3 合入，已留注释，无假实现）
  - 测试：services/shared 107 + circuit-plugin 10 + bootstrap broker 9（含真实 MCP 链路端到端）

- **circuit:** Circuit IR v1 与 MCP 工具契约（G1） ([41582c0](https://github.com/sekfung/ESCode/commit/41582c0df85a85331b5fcef5c9652350a7fb212a))
  - packages/shared/src/circuit：IR v1、识别草稿（无 confidence）、8 个 MCP 工具请求/响应、错误码
  - 受管模块 circuit-protocol（module/contract/example/test/CONTRACT.md）与 "./circuit" 公开入口
  - 校验测试：合法与非法输入 20 条；confidence 只能由服务端写出

- **circuit:** CircuitService 核心（G2） ([ee4d928](https://github.com/sekfung/ESCode/commit/ee4d9283697a871cca8df22664c8cf558f1c30ad))
  - 确定性解析：.kicad_sch（导线/结点/标签几何连通性、层次子图纸展开）、.kicad_pcb、网表、BOM
  - IR 组装与一致性校验；写入带 baseRevision，落后拒绝 stale_revision；持久化完成后才发 circuit.changed
  - 源文件 watch → 只翻转 stale（revision 不变）；重新解析按 KiCad UUID 重新套用修正，套不上的标 orphaned
  - 渲染几何走只读 CircuitLayout：与 IR 同 revision 对齐、不进 IR、不参与修正
  - 受管模块 circuit-service（domain/app/adapters）注册进 Local Host；远程 workspace 的 Host 同样注册
  - fixtures：3 块许可宽松的开源 KiCad 板子（来源与许可证见 fixtures/circuit/README.md）
  - 测试 12 个文件 91 个用例（schema 20 + services 71），含事件顺序、并发写、修正重套用、真实板子端到端

- **rust:** 补齐 legacy 会话方法、副屏会话、assistant feedback 与工作流 run 命令 ([25084e6](https://github.com/sekfung/ESCode/commit/25084e6726828a839a1c7328c73c196c5c6bd803))
  - 将 session/create/send/setModel/setThoughtLevel/setMode/close 翻译为 V4 命令走同一写路径，
  - 新增 selection side session：createSelectionSideSession 命令、受限命令守卫、首轮 modelChange marker
  - 新增 setAssistantFeedback 命令
  - 新增工作流 run 取消/恢复命令（run.cancel/run.resume），转发给工作流宿主后台执行并应答
  - TS 侧把 saved workflow 的解析/校验/编译/提交段抽到 dynamic-workflow-saved-launch.ts，
  - 补充 rust-legacy-session-methods / rust-v4-command-gaps 两个 spec 与差分测试

- **rust:** 对齐 v3.15.1 Node runtime ([aff7afe](https://github.com/sekfung/ESCode/commit/aff7afe20c4f39e53dc245883fc6447a304b25bc))
  - 子代理目录改为 agent_listing_delta 增量提醒，Agent 描述静态化
  - guarded 权限模式（匹配器逐位移植，1353 条 TS 语料一致）
  - ReplyToChannel 工具（bot/channel/reply）、FillWorkflowHole、工作流按需激活
  - 对话引用 / 机器人群材料、Gen UI 目录、ReadSessionContext topic 策略
  - turnHeader 计时与输出 token、Bash hint 行剥离与解析边界
  - 内置 Claude 官方市场、插件 MCP 变量展开与 plugin_variable_missing 诊断
  - workflow actor 插件技能、run-launched 子代理权限模式、事件分页
  - node_repl 用过的 browser 轮尾收到 turnEnded

- **rust:** 恢复会话的 shell 提醒对齐 Node；修复 fork child 协作模式（两侧） ([fa0f588](https://github.com/sekfung/ESCode/commit/fa0f5882f3143ba1bc618ea50fa2accc2d1f6027))
  - 冷恢复 / fork / 副屏 child 在本进程首轮按 TS 规则注入 `The Bash tool shell is …`，只进请求、不落库
  - fork child 按被选轮的模式执行：Rust 回复边界记录 mode/planEnabled；Node 修复 child 行 permission
  - 新增差分用例 shell-resume-notice、fork-child-mode；fork / 副屏用例不再过滤 shell 提醒

- **rust:** 模型请求带上与 Node 一致的客户端头与归因头 ([5c9668b](https://github.com/sekfung/ESCode/commit/5c9668bfbbb53c9e37e5dfd345b1b7682de7ff4c))
  - 静态头：http-referer、user-agent、x-title、x-release-channel、x-client-language/timezone、x-zcode-agent/app-version、x-platform、x-os-\*
  - 每次尝试的归因头：x-request-id、x-zcode-session-type、x-zcode-trace-id、x-session-id、x-query-id、x-opencode-session
  - 合并顺序：客户端头 → Provider 配置头 → 归因头 → 鉴权，同名覆盖不重复
  - 新增差分用例 model-headers

- **rust:** 配置文件 network 段按出口作用域生效，Windows 工具输出按旧代码页解码 ([355ad37](https://github.com/sekfung/ESCode/commit/355ad37e379b70c6d5d12d0946d5b6c47f885927))
  - host::net_config 持有 user / workspace 两个合并视图的 network 段：模型与 MCP 用进程级（user + env），
  - host::output_encoding 对齐 TS outputEncoding：非法 UTF-8 在 Windows 上按 OEM / locale 推断的代码页解码
  - 差分测试覆盖 user / project 配置对 MCP、Bash 子进程环境与 WebFetch 的影响

- **rust:** 实现 V4 amendWorkflowRunSettings 工作流运行设置修订 ([92e7e50](https://github.com/sekfung/ESCode/commit/92e7e503ae792b28c3f05caa9b68ac34076036a8))

- **rust:** 提供与 Node 一致的 -p 无头模式 ([bcd0c03](https://github.com/sekfung/ESCode/commit/bcd0c03b9af788f2ae27071590086a509dadfcdb))
  - 参数解析对齐 Node util.parseArgs（strict）与 run.ts 的校验文本；text / json 输出与键序一致
  - 进程内驱动与 app-server 同一个 Engine；缺省 yolo，需要审批的工具按 Node deny broker 拒绝，不生成标题
  - 本轮用量按 usage.delta 遥测累加；-c / --resume 续接；SIGINT / SIGTERM 退出码 130 / 143
  - app-server 与 -p 共用 runtime 装配（工作区、TS 历史导入、Engine 构建）
  - stream-json 与 --target 等选项本期明确报不支持；新增差分用例 headless

- **rust:** 中枢直接启动已保存工作流（startSavedWorkflow） ([a844a7d](https://github.com/sekfung/ESCode/commit/a844a7db6df9a62f7e0eeb74d5dff1d9fc363cde))

- **rust:** 重试原因码映射为 fault.\*，模型断流后丢弃尾部并重发恢复 ([86d38f2](https://github.com/sekfung/ESCode/commit/86d38f206424d93741909126df1360b91d77393d))
  - apiRetry.reasonCode 按 TS modelRetryReasonCode 映射；旧 session/read 的 error 仍给原始 reason
  - 已有可见输出后断流：作废流式尾部（正文 interrupted、已接正文的推理 complete），同一历史重发，每个 run 至多 10 次
  - 新增差分用例 stream-recovery；更新 Rust 用例到新语义

- **rust:** forkAssistant 落子会话身份与分支边界产物 ([7f16e40](https://github.com/sekfung/ESCode/commit/7f16e40e6d2ba77acdd58217ae2a77b5922c370d))

- **rust:** runtime 写 JSONL 文件日志并做 7 天保留 ([eb9216c](https://github.com/sekfung/ESCode/commit/eb9216c6f18a706c9cf2962f4f75aac4d951e14b))
  - host::file_log：~/.zcode/cli/log/zcode-rust-YYYY-MM-DD.jsonl，条目与 Node toSerializableEntry 同形，同款脱敏
  - 启动 60 秒后清理本 runtime 7 天前的日文件；进程生命周期与原 stderr 诊断改经 file_log（warn/error 仍写 stderr）
  - 桌面导出日志收集同一目录，Rust 作为 runtime 时反馈里有 runtime 日志

- **rust:** session/read 的会话投影对齐 Node 的 mapSessionInfo ([b216770](https://github.com/sekfung/ESCode/commit/b21677055afc53afacf027bbce6cb7ea1c546179))
  - 补 traceId：进程内会话与 session/list 一样投影 s.trace_id（TS session.traceID ?? app.traceId）。
  - 补 target：无 Goal 时显式 null，不是缺字段（TS mapSessionGoal 对无目标返回 null）。
  - session.model 只发 {providerId, modelId}：TS 从 optionalModelSelectionFromString(getModel())
  - 新会话不发 titleSource：TS 直发会话记录字段，新建会话时它还没设置（整个键消失）；Rust 的内部

- **rust:** session/read 有 Goal 的会话投影 session.target ([4cb09d4](https://github.com/sekfung/ESCode/commit/4cb09d4e981dc070e09d03c607fee74ed15171c1))

- **serial:** 波形绘图（第三期 6/6） ([e9cabef](https://github.com/sekfung/ESCode/commit/e9cabefc8d5bc3799e51e5fe6d8403f31531b88a))
  - 显示模式新增“波形”：按行解析数字列或 名称:值/名称=值，最多 8 条曲线、每条 2000 点
  - 增量解析（按 seq 只处理新 chunk），重绘节流为每秒最多 10 次
  - 复用日志工具栏的暂停、清空与导出；波形模式导出 CSV

- **serial:** 定时循环发送（第三期 4/6） ([5107c93](https://github.com/sekfung/ESCode/commit/5107c936a7bba92e3fcbb43e9b99161bd22df41b))
  - 会话内 SerialLoopRunner 由 Host 调度：启动即发送一次，之后按间隔（≥10ms）发送，
  - 进度挂在状态 loop 上，进度事件不超过每 250ms 一次；共享协议状态 schema 同步 loop
  - 发送栏“循环”开关（间隔、次数），快捷指令菜单“循环发送此指令”，状态行显示进度
  - 修复：关闭或断开时被取消的在途写入会迟到地发出 error 事件，移除监听后成为未捕获异常

- **serial:** 多串口（第三期 1/6） ([7e88868](https://github.com/sekfung/ESCode/commit/7e88868f15f196ee229bf113376b66af4254e35a))
  - SerialService 按串口路径管理会话（SerialSession），窗口内最多同时 4 个活动会话；
  - seq 由 Service 在窗口内统一分配，跨会话与关闭重开都不回退；关闭的会话保留历史（最多 8 个）
  - 一次热插拔轮询服务所有会话的重连
  - Agent 工具增加可选 path：恰好一个会话时可省略，多个会话时要求指定；serial_list 返回 sessions
  - 面板标签可多开，每个标签绑定一个串口（标题为串口名），可挂回运行中的会话；
  - 修复：新标签预选在会话列表加载前执行，误把已占用串口当成空闲（两份列表一起加载后再预选）

- **serial:** 快捷指令（第三期 3/6） ([53ac03e](https://github.com/sekfung/ESCode/commit/53ac03e25cef2b03e09ae891149f8d1987d753e2))
  - 全局设置 serialQuickCommands（最多 100 条）：名称、内容、文本/HEX、行尾，所有串口共用
  - 面板快捷指令栏：点击即发送，拖拽排序（@dnd-kit/sortable），新增/编辑/删除
  - 导入/导出 JSON：导出不含 id；导入追加并重新分配 id，非法条目跳过、超过上限丢弃并提示数量
  - 列表操作与导入校验为纯函数并有单测

- **serial:** 校验和与接收分帧（第三期 5/6） ([c0c3279](https://github.com/sekfung/ESCode/commit/c0c327939ccad109b11f7150ccb5f1f5cd2a876c))
  - shared：XOR/SUM8/CRC-8/CRC-16 MODBUS/CCITT-FALSE/CRC-32/LRC，支持跳过字节与字节序；
  - 快捷指令可携带 checksum 配置，随设置校验与导入导出保留
  - 发送栏与快捷指令编辑器复用校验和选择器
  - 接收分帧（分隔符/固定长度/时间间隔）与帧校验只在渲染层生效，失败帧标红并显示期望值，

- **serial:** Agent 串口工具（第二期：内置 serial 插件 + TS/Rust broker） ([6887398](https://github.com/sekfung/ESCode/commit/6887398f93a6347c9a6a4ee9340cf6e836c86050))
  - 内置官方插件 serial（apps/zcode-cli/packages/serial-plugin）：MCP server 提供
  - Agent 运行时 broker（TS bootstrap 与 Rust tools 同构）：私有管道 + token，拒绝子 agent，
  - Host：serialAgentRequestRouter + SerialAgentBridge 直接调用同进程 SerialService，
  - 仅 desktop-local Host 注入 ZCODE_HOST_SERIAL=1，运行时据此注册 serial server
  - SerialService：Agent 写入带 sessionId、write 返回 seq、新增 readSince / waitFor
  - 编解码移入 @zcode/shared/serial，面板、审批预览与 Host 共用
  - UI：审批卡片显示串口写入预览；面板 Agent 行标注 [Agent·会话标题] 并可跳转
  - Rust HostReply 错误保留 error.data；broker 监听抽成通用实现
  - 修复：serial server 注册固定 protocolVersion 2026-07-28；CLI 打包补 @zcode/shared/serial 别名

- **serial:** Desktop 内置串口调试器（第一期：人用面板） ([b891011](https://github.com/sekfung/ESCode/commit/b891011489cd1b32e20c34fb2bc8faef3820467c))
  - SerialService 只注册在 Desktop Local Host，是串口会话的唯一所有者：
  - serialport 原生模块延迟加载，失败时降级为 nativeUnavailable；
  - 侧边面板新增 serial 标签（跟随窗口，在任意 workspace/对话中可见），
  - 面板支持文本/HEX 收发、UTF-8/GBK 显示、时间戳、暂停、清屏、导出，
  - E2E 接缝：ZCODE_SERIAL_MOCK_PORTS 启用 binding-mock 回环虚拟串口

- **serial:** DTR/RTS 手动控制与复位脉冲（第三期 2/6） ([1ffc007](https://github.com/sekfung/ESCode/commit/1ffc007b3e75364619a2b028c1a987b80b04b194))
  - 会话持有 DTR/RTS 状态（打开后初值 1/1），随状态事件广播；自动重连后恢复断开前的状态
  - setSignals 与打开/关闭在会话内串行；复位脉冲预设 ESP32（esptool 经典复位）与 Arduino，
  - Agent 新增 serial_set_signals（需审批）与 interaction/serialSetSignals，TS 与 Rust broker 同步；
  - 面板新增 DTR/RTS 开关与复位下拉
  - 修复：状态新增 signals 后共享协议的 strict 状态 schema 会整条拒收，已同步契约

- **ui:** 会话工具条显示实时 token 生成速度 ([158adcb](https://github.com/sekfung/ESCode/commit/158adcb398d7377f96b89f0153a2234f4a128111))
  - Renderer 本地观测器按 3 秒滑动窗口估算 tok/s，只采信 online 投递，recovery/resync 记为缺口
  - 工具执行与等待授权/回答的区间不计入本轮平均，后台化工具不阻塞；轮末用 turnHeader.outputTokens 修正分子
  - 读数落在输入框工具条上下文容量左侧，独立于 modelLocked，桌面与手机远控共用同一实现
  - estimateTokens 上移到 packages/shared，CLI 上下文预算与读数共用同一估算口径
  - 规格 docs/specs/conversation-generation-speed.md、决策 docs/adr/0006、单测 18 项与 E2E 候选

- update to v3.15.1 ([aac4755](https://github.com/sekfung/ESCode/commit/aac4755666d09fdcd70272fcf063c077a639015f))
  - Add UI plugins and inline Gen UI experiences
  - Update desktop, remote workspace, and agent capabilities
  - Expand built-in plugins and regression coverage

- update v3.14.3 ([29628c9](https://github.com/sekfung/ESCode/commit/29628c9acdb81b703bbd4080c207a0e7ce5e276e))
  - The concurrency limit of a running workflow can now be adjusted directly, without stopping the task.
  - Optimized the reuse logic when modifying and restarting workflows.
  - Improved the real-time status display for large workflows.
  - Improved the efficiency of workflow script submission and modification, reducing token consumption.
  - Fixed an issue where workflows could cause the interface to crash in some cases.
  - Fixed an issue where buttons on workflow cards were sometimes pushed out of the interface.
  - Fixed an issue where the workflow tool took up too much context.

### Bug Fixes

- **circuit:** broker 免批准名单断言补 submit_recognition ([bc3ddb7](https://github.com/sekfung/ESCode/commit/bc3ddb75e9464a3530d1681f0891af0d0bbfd395))

- **circuit:** CLI 构建补 @escode/shared/circuit 精确 alias ([2bc832e](https://github.com/sekfung/ESCode/commit/2bc832e074c3389b81893581cb2f53cb755de1e8))

- **circuit:** CLI 构建补 @escode/shared/circuit 精确 alias ([31e00e0](https://github.com/sekfung/ESCode/commit/31e00e09a8367d2957b117f0cdddfa01fff0ac24))

- **desktop:** Gen UI 供应商资源固定 LF 换行 ([f28e193](https://github.com/sekfung/ESCode/commit/f28e19339b66deeb6f288bdb2423d07145b0b93c))

- **model:** 对齐严格实现的 Responses/Anthropic 请求形态 ([1b14a92](https://github.com/sekfung/ESCode/commit/1b14a9239baec74c552806f5e8bbdf1b9a3f4e3b))

- **plugins:** DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS 补 serial ([4bc300d](https://github.com/sekfung/ESCode/commit/4bc300dc72548942749eff43b2ed7d778037482a))

- **rust:** 复查补齐 visualize 技能门禁与 session-context 跳过集 ([5d7b2bb](https://github.com/sekfung/ESCode/commit/5d7b2bbc78ce55248d5da2492320ca9b52558617))
  - visualize 技能门禁：协议入口保留官方 visualize 技能、`-p` 无头入口过滤
  - session-context 合成提醒跳过集 9 → 11 条（memory_update / relevant_memory）；
  - drift 文档新增缺口：highspeed 加速卡按「不跟进」登记（不接 Z.ai 套餐，凭据链路
  - 同步修正 rust-parity-remaining / rust-critical-parity / rust-hooks /

- **rust:** 工作区锁文件打不开时把锁路径带进错误 ([ced64f1](https://github.com/sekfung/ESCode/commit/ced64f12313a15c69a49825c1710f665440349fc))

- **rust:** 客户端时区头把 UTC 别名归一为 UTC，与 V8 一致 ([4e3f44b](https://github.com/sekfung/ESCode/commit/4e3f44be7bb4b1c8d7d4f0134aa2a14883bb6a0e))

- **rust:** 修复非 Windows clippy 死代码与两条 CI 偶发差分用例 ([af8c3c4](https://github.com/sekfung/ESCode/commit/af8c3c4a99c73bd5aa9f68267bfa16bd1b182cc7))
  - output_encoding 的代码页辅助只在 Windows 用到，非 Windows 允许 dead_code
  - amend-workflow-settings：3 核 runner 上并发天花板为 1，就地调并发必然 unchanged，按天花板决定期望
  - start-saved-workflow：启动后 session.status 读到 running 或 idle 取决于调度，只校验落在合法集合内

- **rust:** Bash 解析器在语法边界上对齐 unbash ([b7a3c1a](https://github.com/sekfung/ESCode/commit/b7a3c1a684e6bc8a101f56cc7ebf0b88915cdaf7))
  - 只读语料新增约 130 条语法边界用例进入解析层 oracle
  - 命令位置遇到不能开始命令的 token（`; ls`、`;;`、悬空 `&&`/`|`、管道后 `!`、`}`）静默停止；前导 `!` 按取反跳过
  - 已用真实 bash 核对这些输入整行不执行，不放宽任何实际执行的命令

- **ui:** 只带电路引用时 composer 不渲染 chip 行（G5 缺陷） ([36df198](https://github.com/sekfung/ESCode/commit/36df1989fa8e4822fbe8d35aa6741f7bae63c4cc))

- **workflow-host:** 停止 run 时等结算通知写出后再应答 ([5b78d41](https://github.com/sekfung/ESCode/commit/5b78d4111fca3121e6d7d2116b45132d271d97ec))

### Chores

- 固定 Electron 下载镜像到 npmmirror，README 补充安装说明 ([caa6946](https://github.com/sekfung/ESCode/commit/caa6946695f0dbb2796331c2ab8f0a515b41ae8a))
  - .npmrc 增加 electron_mirror，与 mise.toml 的 ELECTRON_MIRROR 及
  - README.md / README.en.md 说明该行为、改用其他镜像的位置，以及项目级配置
  - 顺带删除 README.md 标题区多余空行（oxfmt 要求）

- update gitignore ([7ad288e](https://github.com/sekfung/ESCode/commit/7ad288e72f7ffa9c7ca439e1fd4328c0f2798c8d))

### Documentation

- **circuit:** 电路识别 P1 规格与 ADR 0005 ([84aa778](https://github.com/sekfung/ESCode/commit/84aa7788a41421ca7ad7d31ed5b215389c5bc91e))

- **rebrand:** 更名收尾——自有文档同步、第三方清单路径与安装图标 ([ce0f758](https://github.com/sekfung/ESCode/commit/ce0f758fe03fe9116d0fda423a410e46fd55377e))
  - 自有文档同步 ESCode 现状：docs/electron 4 篇与 third-party README/SPEC 的
  - copied-components.json 三处旧路径改为实际路径；generate-third-party-notices
  - third-party/README 记录已知问题：licenses.mjs 在钉住的 pnpm 10.33.2 上无法运行
  - 安装素材 icon_installer.\* 重新生成为浅底变体（此前与应用图标逐字节同源，
  - escode-rebrand spec：保留清单口径收紧为镜像目录（docs/electron/docs/\*\*、

- **rust:** 更新 V4 命令缺口记录与三处差分结论 ([9a9a9fd](https://github.com/sekfung/ESCode/commit/9a9a9fdbffbc286fbb09520968f99632887a79d9))
  - 表格：startSavedWorkflow 转「已实现，差分一致」；setAssistantFeedback /
  - 新增 forkAssistant 子会话身份与边界、session/read 的 session 投影两节，含已知差异
  - startSavedWorkflow 一节记三方所有权、三段时序图与拒绝词表。

- **rust:** 记录 2026-10-10 发布回退复验结果 ([ddbf0e9](https://github.com/sekfung/ESCode/commit/ddbf0e96ab4ac2ab0fed036c9c5d680f6ed02d45))

- **rust:** 跨平台真机与发布回退验收记录；发布资产比对按 GitHub 命名规则归一 ([444d4a9](https://github.com/sekfung/ESCode/commit/444d4a932743bf1847798b573ff010949d92de21))

### Refactorings

- 产品更名为 ESCode ([13c5a1a](https://github.com/sekfung/ESCode/commit/13c5a1a2dd79cb4493e2a874f8c02665354465c9))
  - 包名、目录、crate、环境变量、协议名与深链 scheme 改为 ESCode
  - 图标、dmg 背景与应用内标识替换为 ESCode 标识
  - 上游 NOTICE 原文移至 third-party/upstream-zcode/，新 NOTICE 声明来源与修改
  - 保留外部服务主机名、插件市场 id 与旧插件清单目录回退（docs/specs/escode-rebrand.md）
  - 修复更名引入的测试：About 标识 viewBox、官方市场展示名（不再暴露 zcode-plugins-official
  - 补齐批量替换漏掉的旧名写法：Z Code（退出确认文案、X-Title 客户端标识、文档标题）与
  - 验证：typecheck / lint / architecture:check / 30 个漂移生成器 --check / cargo test（319 通过）

- **rust:** 拆出 shell 提醒模块，满足单文件 400 行边界 ([1bc0321](https://github.com/sekfung/ESCode/commit/1bc0321b1ce1d2b79572ab3b439afb7be7e13dfc))
