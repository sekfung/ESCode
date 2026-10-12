# Changelog

## [3.16.0](https://github.com/sekfung/ESCode/compare/v3.14.0-rust-rc.1...v3.16.0) (2026-10-12)

### Features

* 桌面端默认 runtime 切换为 Rust ([7950284](https://github.com/sekfung/ESCode/commit/7950284d339835c0663c4daf14713e78fbfe13b3))

* **circuit:** 电路面板、引用卡片与入口（G5） ([58954ab](https://github.com/sekfung/ESCode/commit/58954abd436ae53ab948e74d8c3301d710f2ac66))
  * hooks：useCircuitPanel（服务访问、按 (revision, stale) 比对刷新、stale/orphaned/写冲突状态）、useCircuitDraftReferences
  * 面板：layout 自绘 SVG（shapes/wires/junctions/labels/pins/texts，多图纸切换）、点选器件/网络高亮、
  * 入口：文件树 .kicad_sch/.kicad_pcb/.circuit.json 路由；::circuit 卡片（Gen UI 解析规则）打开面板定位 IR；
  * 只读查询扩展：ref/net/block 过滤全部省略时列出该类全部（BOM/网络表要枚举事实来源），id 与名字同时给出仍拒绝
  * 平台能力位新增 supportsCircuit（Desktop true，Web/手机 undefined），与 supportsSerialPort 同构
  * 测试：ui 4 文件 56 个；电路相关回归 18 文件 193 个
  * E2E 用例 docs/test-cases/e2e/circuit-panel.feature（7 个场景）；wdio 实跑受环境阻塞，详见交接说明

* **circuit:** 接入 circuit_check 与 circuit_compare_firmware 工具（G4 收尾） ([08cecb3](https://github.com/sekfung/ESCode/commit/08cecb3b68bed774724ce3bf61469be0bf1497e0))
  * 7 个 op 全部打通（parse/get/query/correct/check/compareFirmware/export）；submit_recognition 仍留给 G6
  * shared：broker 信封与 interaction/circuitCheck、interaction/circuitCompareFirmware 反向方法；协议方法表补两项
  * compare_firmware 请求补可选 cursor（此前 nextCursor 是死字段）
  * bridge/router：check/compareFirmware → 服务方法，cursor/files 透传，错误码沿用既有映射
  * 插件：两个只读工具注册；skill 补「规则结果与模型建议分开」「固件比对用于联调」，
  * 审批名单：check/compareFirmware 为只读放行，export 仍需批准
  * 测试：services+shared 15 文件 136 个、circuit-plugin 12 个、bootstrap broker 11 个

* **circuit:** 内置规则检查与固件比对（G3） ([5bc2687](https://github.com/sekfung/ESCode/commit/5bc2687014c9936f0d6aeebd9f9cd80c8ccc75ce))
  * 6 条内置规则（unconnected-pin / single-pin-net / undriven-power / i2c-missing-pullup /
  * .escode/circuit-rules.json 只读配置：启用/停用与级别覆盖；不合法整份忽略并报告 invalid
  * 固件比对：devicetree（.dts/.dtsi/.overlay）、STM32CubeMX .ioc、C 头文件引脚宏，四类差异带来源行号
  * 规格补 §6.1 配置格式 / §6.2 判定口径与 P1 边界 / §7.1 固件解析范围（IR 无电气类型与坐标，用确定性代理）
  * fixtures：每条规则一对 hit/miss 最小原理图 + 手写固件样例（埋 4 处不一致）
  * 测试：规则 12 + 配置 16 + 固件 15 + 契约 6（circuit 模块合计 116 通过）

* **circuit:** 识别提交与离线评测（G6） ([d2d6198](https://github.com/sekfung/ESCode/commit/d2d619873690390dae21f15e130d8b51b21be93b))
  * circuit_submit_recognition：服务端 schema + 一致性校验，最终元素 id 由服务端生成并重映射草稿内引用
  * 置信度只由服务端给出：基准 0.9 按校验扣分、夹在 [0.05, 0.9]、永不等于 1；矢量 PDF 文本证据命中记 1
  * 矢量 PDF 优先直接提取文本与线段（pdfjs-dist：CTM 跟踪、q/Q/cm、左上角原点，与面板叠框坐标一致）
  * 图片/PDF 按内容哈希派生 irId：内容一变即新 IR（新 id、revision 从 1、旧修正不迁移）
  * 同内容重新提交沿用 parse 前置条件（缺 baseRevision → invalid_ir，落后 → stale_revision），为此 schema 增可选 baseRevision
  * 浮点四舍五入到 3 位，避免浮点尾数写入 IR
  * 工具链路：broker/桥接/插件/skill 全部接入（免批准）；skill 补「先告知用户再发图 / 矢量 PDF 优先 / 低置信度必须说明」
  * 离线评测脚本 scripts/circuit-recognition-eval.mjs（本机 kicad-cli 导出语料，缺失即报错并给安装/手工放置说明）
  * 度量纯函数与单测（不进 CI 门禁）；报告写明模型标签、样本数、匹配口径与未匹配清单
  * 测试：services circuit 16 文件 149 个、整链路 8 个、plugin 13 个、shared/metrics 40 个

* **circuit:** broker、内置插件与远程访问（G4） ([257fd76](https://github.com/sekfung/ESCode/commit/257fd76cd461c9401ea0d5828d11e2b0bede7d82))
  * Host 侧：circuitAgentBridge（工具→服务方法、错误码映射）、circuitAgentRequestRouter（interaction/circuit* 严格校验与响应复核）
  * 工具：circuit_parse/get/query/correct/export；export 走权限审批（不在 defaultAllowedTools）
  * 服务契约新增 exportSnapshot（快照为派生产物，服务不读回，§2 规则 8）
  * CLI 侧：进程级 circuit broker、bootstrap 反向控制通道、circuit-plugin（MCP server + circuit skill）注册为官方插件
  * shared：circuit broker 信封与反向方法、escode-protocol 方法表；Rust official_plugins.json 重新生成
  * 远程访问：remoteServiceAccess 接入 CircuitService，贯穿 workspaceIdentity/remoteSessionId
  * 未注册 circuit_submit_recognition（G6）与 circuit_check / circuit_compare_firmware（等 G3 合入，已留注释，无假实现）
  * 测试：services/shared 107 + circuit-plugin 10 + bootstrap broker 9（含真实 MCP 链路端到端）

* **circuit:** Circuit IR v1 与 MCP 工具契约（G1） ([41582c0](https://github.com/sekfung/ESCode/commit/41582c0df85a85331b5fcef5c9652350a7fb212a))
  * packages/shared/src/circuit：IR v1、识别草稿（无 confidence）、8 个 MCP 工具请求/响应、错误码
  * 受管模块 circuit-protocol（module/contract/example/test/CONTRACT.md）与 "./circuit" 公开入口
  * 校验测试：合法与非法输入 20 条；confidence 只能由服务端写出

* **circuit:** CircuitService 核心（G2） ([ee4d928](https://github.com/sekfung/ESCode/commit/ee4d9283697a871cca8df22664c8cf558f1c30ad))
  * 确定性解析：.kicad_sch（导线/结点/标签几何连通性、层次子图纸展开）、.kicad_pcb、网表、BOM
  * IR 组装与一致性校验；写入带 baseRevision，落后拒绝 stale_revision；持久化完成后才发 circuit.changed
  * 源文件 watch → 只翻转 stale（revision 不变）；重新解析按 KiCad UUID 重新套用修正，套不上的标 orphaned
  * 渲染几何走只读 CircuitLayout：与 IR 同 revision 对齐、不进 IR、不参与修正
  * 受管模块 circuit-service（domain/app/adapters）注册进 Local Host；远程 workspace 的 Host 同样注册
  * fixtures：3 块许可宽松的开源 KiCad 板子（来源与许可证见 fixtures/circuit/README.md）
  * 测试 12 个文件 91 个用例（schema 20 + services 71），含事件顺序、并发写、修正重套用、真实板子端到端

* **rust:** 补齐 legacy 会话方法、副屏会话、assistant feedback 与工作流 run 命令 ([25084e6](https://github.com/sekfung/ESCode/commit/25084e6726828a839a1c7328c73c196c5c6bd803))
  * 将 session/create/send/setModel/setThoughtLevel/setMode/close 翻译为 V4 命令走同一写路径，
  * 新增 selection side session：createSelectionSideSession 命令、受限命令守卫、首轮 modelChange marker
  * 新增 setAssistantFeedback 命令
  * 新增工作流 run 取消/恢复命令（run.cancel/run.resume），转发给工作流宿主后台执行并应答
  * TS 侧把 saved workflow 的解析/校验/编译/提交段抽到 dynamic-workflow-saved-launch.ts，
  * 补充 rust-legacy-session-methods / rust-v4-command-gaps 两个 spec 与差分测试

* **rust:** 对齐 v3.15.1 Node runtime ([aff7afe](https://github.com/sekfung/ESCode/commit/aff7afe20c4f39e53dc245883fc6447a304b25bc))
  * 子代理目录改为 agent_listing_delta 增量提醒，Agent 描述静态化
  * guarded 权限模式（匹配器逐位移植，1353 条 TS 语料一致）
  * ReplyToChannel 工具（bot/channel/reply）、FillWorkflowHole、工作流按需激活
  * 对话引用 / 机器人群材料、Gen UI 目录、ReadSessionContext topic 策略
  * turnHeader 计时与输出 token、Bash hint 行剥离与解析边界
  * 内置 Claude 官方市场、插件 MCP 变量展开与 plugin_variable_missing 诊断
  * workflow actor 插件技能、run-launched 子代理权限模式、事件分页
  * node_repl 用过的 browser 轮尾收到 turnEnded

* **rust:** 恢复会话的 shell 提醒对齐 Node；修复 fork child 协作模式（两侧） ([fa0f588](https://github.com/sekfung/ESCode/commit/fa0f5882f3143ba1bc618ea50fa2accc2d1f6027))
  * 冷恢复 / fork / 副屏 child 在本进程首轮按 TS 规则注入 `The Bash tool shell is …`，只进请求、不落库
  * fork child 按被选轮的模式执行：Rust 回复边界记录 mode/planEnabled；Node 修复 child 行 permission
  * 新增差分用例 shell-resume-notice、fork-child-mode；fork / 副屏用例不再过滤 shell 提醒

* **rust:** 模型请求带上与 Node 一致的客户端头与归因头 ([5c9668b](https://github.com/sekfung/ESCode/commit/5c9668bfbbb53c9e37e5dfd345b1b7682de7ff4c))
  * 静态头：http-referer、user-agent、x-title、x-release-channel、x-client-language/timezone、x-zcode-agent/app-version、x-platform、x-os-*
  * 每次尝试的归因头：x-request-id、x-zcode-session-type、x-zcode-trace-id、x-session-id、x-query-id、x-opencode-session
  * 合并顺序：客户端头 → Provider 配置头 → 归因头 → 鉴权，同名覆盖不重复
  * 新增差分用例 model-headers

* **rust:** 配置文件 network 段按出口作用域生效，Windows 工具输出按旧代码页解码 ([355ad37](https://github.com/sekfung/ESCode/commit/355ad37e379b70c6d5d12d0946d5b6c47f885927))
  * host::net_config 持有 user / workspace 两个合并视图的 network 段：模型与 MCP 用进程级（user + env），
  * host::output_encoding 对齐 TS outputEncoding：非法 UTF-8 在 Windows 上按 OEM / locale 推断的代码页解码
  * 差分测试覆盖 user / project 配置对 MCP、Bash 子进程环境与 WebFetch 的影响

* **rust:** 实现 V4 amendWorkflowRunSettings 工作流运行设置修订 ([92e7e50](https://github.com/sekfung/ESCode/commit/92e7e503ae792b28c3f05caa9b68ac34076036a8))

* **rust:** 提供与 Node 一致的 -p 无头模式 ([bcd0c03](https://github.com/sekfung/ESCode/commit/bcd0c03b9af788f2ae27071590086a509dadfcdb))
  * 参数解析对齐 Node util.parseArgs（strict）与 run.ts 的校验文本；text / json 输出与键序一致
  * 进程内驱动与 app-server 同一个 Engine；缺省 yolo，需要审批的工具按 Node deny broker 拒绝，不生成标题
  * 本轮用量按 usage.delta 遥测累加；-c / --resume 续接；SIGINT / SIGTERM 退出码 130 / 143
  * app-server 与 -p 共用 runtime 装配（工作区、TS 历史导入、Engine 构建）
  * stream-json 与 --target 等选项本期明确报不支持；新增差分用例 headless

* **rust:** 中枢直接启动已保存工作流（startSavedWorkflow） ([a844a7d](https://github.com/sekfung/ESCode/commit/a844a7db6df9a62f7e0eeb74d5dff1d9fc363cde))

* **rust:** 重试原因码映射为 fault.*，模型断流后丢弃尾部并重发恢复 ([86d38f2](https://github.com/sekfung/ESCode/commit/86d38f206424d93741909126df1360b91d77393d))
  * apiRetry.reasonCode 按 TS modelRetryReasonCode 映射；旧 session/read 的 error 仍给原始 reason
  * 已有可见输出后断流：作废流式尾部（正文 interrupted、已接正文的推理 complete），同一历史重发，每个 run 至多 10 次
  * 新增差分用例 stream-recovery；更新 Rust 用例到新语义

* **rust:** forkAssistant 落子会话身份与分支边界产物 ([7f16e40](https://github.com/sekfung/ESCode/commit/7f16e40e6d2ba77acdd58217ae2a77b5922c370d))

* **rust:** runtime 写 JSONL 文件日志并做 7 天保留 ([eb9216c](https://github.com/sekfung/ESCode/commit/eb9216c6f18a706c9cf2962f4f75aac4d951e14b))
  * host::file_log：~/.zcode/cli/log/zcode-rust-YYYY-MM-DD.jsonl，条目与 Node toSerializableEntry 同形，同款脱敏
  * 启动 60 秒后清理本 runtime 7 天前的日文件；进程生命周期与原 stderr 诊断改经 file_log（warn/error 仍写 stderr）
  * 桌面导出日志收集同一目录，Rust 作为 runtime 时反馈里有 runtime 日志

* **rust:** session/read 的会话投影对齐 Node 的 mapSessionInfo ([b216770](https://github.com/sekfung/ESCode/commit/b21677055afc53afacf027bbce6cb7ea1c546179))
  * 补 traceId：进程内会话与 session/list 一样投影 s.trace_id（TS session.traceID ?? app.traceId）。
  * 补 target：无 Goal 时显式 null，不是缺字段（TS mapSessionGoal 对无目标返回 null）。
  * session.model 只发 {providerId, modelId}：TS 从 optionalModelSelectionFromString(getModel())
  * 新会话不发 titleSource：TS 直发会话记录字段，新建会话时它还没设置（整个键消失）；Rust 的内部

* **rust:** session/read 有 Goal 的会话投影 session.target ([4cb09d4](https://github.com/sekfung/ESCode/commit/4cb09d4e981dc070e09d03c607fee74ed15171c1))

* **serial:** 波形绘图（第三期 6/6） ([e9cabef](https://github.com/sekfung/ESCode/commit/e9cabefc8d5bc3799e51e5fe6d8403f31531b88a))
  * 显示模式新增“波形”：按行解析数字列或 名称:值/名称=值，最多 8 条曲线、每条 2000 点
  * 增量解析（按 seq 只处理新 chunk），重绘节流为每秒最多 10 次
  * 复用日志工具栏的暂停、清空与导出；波形模式导出 CSV

* **serial:** 定时循环发送（第三期 4/6） ([5107c93](https://github.com/sekfung/ESCode/commit/5107c936a7bba92e3fcbb43e9b99161bd22df41b))
  * 会话内 SerialLoopRunner 由 Host 调度：启动即发送一次，之后按间隔（≥10ms）发送，
  * 进度挂在状态 loop 上，进度事件不超过每 250ms 一次；共享协议状态 schema 同步 loop
  * 发送栏“循环”开关（间隔、次数），快捷指令菜单“循环发送此指令”，状态行显示进度
  * 修复：关闭或断开时被取消的在途写入会迟到地发出 error 事件，移除监听后成为未捕获异常

* **serial:** 多串口（第三期 1/6） ([7e88868](https://github.com/sekfung/ESCode/commit/7e88868f15f196ee229bf113376b66af4254e35a))
  * SerialService 按串口路径管理会话（SerialSession），窗口内最多同时 4 个活动会话；
  * seq 由 Service 在窗口内统一分配，跨会话与关闭重开都不回退；关闭的会话保留历史（最多 8 个）
  * 一次热插拔轮询服务所有会话的重连
  * Agent 工具增加可选 path：恰好一个会话时可省略，多个会话时要求指定；serial_list 返回 sessions
  * 面板标签可多开，每个标签绑定一个串口（标题为串口名），可挂回运行中的会话；
  * 修复：新标签预选在会话列表加载前执行，误把已占用串口当成空闲（两份列表一起加载后再预选）

* **serial:** 快捷指令（第三期 3/6） ([53ac03e](https://github.com/sekfung/ESCode/commit/53ac03e25cef2b03e09ae891149f8d1987d753e2))
  * 全局设置 serialQuickCommands（最多 100 条）：名称、内容、文本/HEX、行尾，所有串口共用
  * 面板快捷指令栏：点击即发送，拖拽排序（@dnd-kit/sortable），新增/编辑/删除
  * 导入/导出 JSON：导出不含 id；导入追加并重新分配 id，非法条目跳过、超过上限丢弃并提示数量
  * 列表操作与导入校验为纯函数并有单测

* **serial:** 校验和与接收分帧（第三期 5/6） ([c0c3279](https://github.com/sekfung/ESCode/commit/c0c327939ccad109b11f7150ccb5f1f5cd2a876c))
  * shared：XOR/SUM8/CRC-8/CRC-16 MODBUS/CCITT-FALSE/CRC-32/LRC，支持跳过字节与字节序；
  * 快捷指令可携带 checksum 配置，随设置校验与导入导出保留
  * 发送栏与快捷指令编辑器复用校验和选择器
  * 接收分帧（分隔符/固定长度/时间间隔）与帧校验只在渲染层生效，失败帧标红并显示期望值，

* **serial:** Agent 串口工具（第二期：内置 serial 插件 + TS/Rust broker） ([6887398](https://github.com/sekfung/ESCode/commit/6887398f93a6347c9a6a4ee9340cf6e836c86050))
  * 内置官方插件 serial（apps/zcode-cli/packages/serial-plugin）：MCP server 提供
  * Agent 运行时 broker（TS bootstrap 与 Rust tools 同构）：私有管道 + token，拒绝子 agent，
  * Host：serialAgentRequestRouter + SerialAgentBridge 直接调用同进程 SerialService，
  * 仅 desktop-local Host 注入 ZCODE_HOST_SERIAL=1，运行时据此注册 serial server
  * SerialService：Agent 写入带 sessionId、write 返回 seq、新增 readSince / waitFor
  * 编解码移入 @zcode/shared/serial，面板、审批预览与 Host 共用
  * UI：审批卡片显示串口写入预览；面板 Agent 行标注 [Agent·会话标题] 并可跳转
  * Rust HostReply 错误保留 error.data；broker 监听抽成通用实现
  * 修复：serial server 注册固定 protocolVersion 2026-07-28；CLI 打包补 @zcode/shared/serial 别名

* **serial:** Desktop 内置串口调试器（第一期：人用面板） ([b891011](https://github.com/sekfung/ESCode/commit/b891011489cd1b32e20c34fb2bc8faef3820467c))
  * SerialService 只注册在 Desktop Local Host，是串口会话的唯一所有者：
  * serialport 原生模块延迟加载，失败时降级为 nativeUnavailable；
  * 侧边面板新增 serial 标签（跟随窗口，在任意 workspace/对话中可见），
  * 面板支持文本/HEX 收发、UTF-8/GBK 显示、时间戳、暂停、清屏、导出，
  * E2E 接缝：ZCODE_SERIAL_MOCK_PORTS 启用 binding-mock 回环虚拟串口

* **serial:** DTR/RTS 手动控制与复位脉冲（第三期 2/6） ([1ffc007](https://github.com/sekfung/ESCode/commit/1ffc007b3e75364619a2b028c1a987b80b04b194))
  * 会话持有 DTR/RTS 状态（打开后初值 1/1），随状态事件广播；自动重连后恢复断开前的状态
  * setSignals 与打开/关闭在会话内串行；复位脉冲预设 ESP32（esptool 经典复位）与 Arduino，
  * Agent 新增 serial_set_signals（需审批）与 interaction/serialSetSignals，TS 与 Rust broker 同步；
  * 面板新增 DTR/RTS 开关与复位下拉
  * 修复：状态新增 signals 后共享协议的 strict 状态 schema 会整条拒收，已同步契约

* **ui:** 会话工具条显示实时 token 生成速度 ([158adcb](https://github.com/sekfung/ESCode/commit/158adcb398d7377f96b89f0153a2234f4a128111))
  * Renderer 本地观测器按 3 秒滑动窗口估算 tok/s，只采信 online 投递，recovery/resync 记为缺口
  * 工具执行与等待授权/回答的区间不计入本轮平均，后台化工具不阻塞；轮末用 turnHeader.outputTokens 修正分子
  * 读数落在输入框工具条上下文容量左侧，独立于 modelLocked，桌面与手机远控共用同一实现
  * estimateTokens 上移到 packages/shared，CLI 上下文预算与读数共用同一估算口径
  * 规格 docs/specs/conversation-generation-speed.md、决策 docs/adr/0006、单测 18 项与 E2E 候选

* update to v3.15.1 ([aac4755](https://github.com/sekfung/ESCode/commit/aac4755666d09fdcd70272fcf063c077a639015f))
  * Add UI plugins and inline Gen UI experiences
  * Update desktop, remote workspace, and agent capabilities
  * Expand built-in plugins and regression coverage

* update v3.14.3 ([29628c9](https://github.com/sekfung/ESCode/commit/29628c9acdb81b703bbd4080c207a0e7ce5e276e))
  * The concurrency limit of a running workflow can now be adjusted directly, without stopping the task.
  * Optimized the reuse logic when modifying and restarting workflows.
  * Improved the real-time status display for large workflows.
  * Improved the efficiency of workflow script submission and modification, reducing token consumption.
  * Fixed an issue where workflows could cause the interface to crash in some cases.
  * Fixed an issue where buttons on workflow cards were sometimes pushed out of the interface.
  * Fixed an issue where the workflow tool took up too much context.


### Bug Fixes

* **circuit:** broker 免批准名单断言补 submit_recognition ([bc3ddb7](https://github.com/sekfung/ESCode/commit/bc3ddb75e9464a3530d1681f0891af0d0bbfd395))

* **circuit:** CLI 构建补 @escode/shared/circuit 精确 alias ([2bc832e](https://github.com/sekfung/ESCode/commit/2bc832e074c3389b81893581cb2f53cb755de1e8))

* **circuit:** CLI 构建补 @escode/shared/circuit 精确 alias ([31e00e0](https://github.com/sekfung/ESCode/commit/31e00e09a8367d2957b117f0cdddfa01fff0ac24))

* **desktop:** Gen UI 供应商资源固定 LF 换行 ([f28e193](https://github.com/sekfung/ESCode/commit/f28e19339b66deeb6f288bdb2423d07145b0b93c))

* **model:** 对齐严格实现的 Responses/Anthropic 请求形态 ([1b14a92](https://github.com/sekfung/ESCode/commit/1b14a9239baec74c552806f5e8bbdf1b9a3f4e3b))

* **plugins:** DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS 补 serial ([4bc300d](https://github.com/sekfung/ESCode/commit/4bc300dc72548942749eff43b2ed7d778037482a))

* **rust:** 复查补齐 visualize 技能门禁与 session-context 跳过集 ([5d7b2bb](https://github.com/sekfung/ESCode/commit/5d7b2bbc78ce55248d5da2492320ca9b52558617))
  * visualize 技能门禁：协议入口保留官方 visualize 技能、`-p` 无头入口过滤
  * session-context 合成提醒跳过集 9 → 11 条（memory_update / relevant_memory）；
  * drift 文档新增缺口：highspeed 加速卡按「不跟进」登记（不接 Z.ai 套餐，凭据链路
  * 同步修正 rust-parity-remaining / rust-critical-parity / rust-hooks /

* **rust:** 工作区锁文件打不开时把锁路径带进错误 ([ced64f1](https://github.com/sekfung/ESCode/commit/ced64f12313a15c69a49825c1710f665440349fc))

* **rust:** 客户端时区头把 UTC 别名归一为 UTC，与 V8 一致 ([4e3f44b](https://github.com/sekfung/ESCode/commit/4e3f44be7bb4b1c8d7d4f0134aa2a14883bb6a0e))

* **rust:** 修复非 Windows clippy 死代码与两条 CI 偶发差分用例 ([af8c3c4](https://github.com/sekfung/ESCode/commit/af8c3c4a99c73bd5aa9f68267bfa16bd1b182cc7))
  * output_encoding 的代码页辅助只在 Windows 用到，非 Windows 允许 dead_code
  * amend-workflow-settings：3 核 runner 上并发天花板为 1，就地调并发必然 unchanged，按天花板决定期望
  * start-saved-workflow：启动后 session.status 读到 running 或 idle 取决于调度，只校验落在合法集合内

* **rust:** Bash 解析器在语法边界上对齐 unbash ([b7a3c1a](https://github.com/sekfung/ESCode/commit/b7a3c1a684e6bc8a101f56cc7ebf0b88915cdaf7))
  * 只读语料新增约 130 条语法边界用例进入解析层 oracle
  * 命令位置遇到不能开始命令的 token（`; ls`、`;;`、悬空 `&&`/`|`、管道后 `!`、`}`）静默停止；前导 `!` 按取反跳过
  * 已用真实 bash 核对这些输入整行不执行，不放宽任何实际执行的命令

* **ui:** 只带电路引用时 composer 不渲染 chip 行（G5 缺陷） ([36df198](https://github.com/sekfung/ESCode/commit/36df1989fa8e4822fbe8d35aa6741f7bae63c4cc))

* **workflow-host:** 停止 run 时等结算通知写出后再应答 ([5b78d41](https://github.com/sekfung/ESCode/commit/5b78d4111fca3121e6d7d2116b45132d271d97ec))


### Chores

* 固定 Electron 下载镜像到 npmmirror，README 补充安装说明 ([caa6946](https://github.com/sekfung/ESCode/commit/caa6946695f0dbb2796331c2ab8f0a515b41ae8a))
  * .npmrc 增加 electron_mirror，与 mise.toml 的 ELECTRON_MIRROR 及
  * README.md / README.en.md 说明该行为、改用其他镜像的位置，以及项目级配置
  * 顺带删除 README.md 标题区多余空行（oxfmt 要求）

* update gitignore ([7ad288e](https://github.com/sekfung/ESCode/commit/7ad288e72f7ffa9c7ca439e1fd4328c0f2798c8d))


### Documentation

* **circuit:** 电路识别 P1 规格与 ADR 0005 ([84aa778](https://github.com/sekfung/ESCode/commit/84aa7788a41421ca7ad7d31ed5b215389c5bc91e))

* **rebrand:** 更名收尾——自有文档同步、第三方清单路径与安装图标 ([ce0f758](https://github.com/sekfung/ESCode/commit/ce0f758fe03fe9116d0fda423a410e46fd55377e))
  * 自有文档同步 ESCode 现状：docs/electron 4 篇与 third-party README/SPEC 的
  * copied-components.json 三处旧路径改为实际路径；generate-third-party-notices
  * third-party/README 记录已知问题：licenses.mjs 在钉住的 pnpm 10.33.2 上无法运行
  * 安装素材 icon_installer.* 重新生成为浅底变体（此前与应用图标逐字节同源，
  * escode-rebrand spec：保留清单口径收紧为镜像目录（docs/electron/docs/**、

* **rust:** 更新 V4 命令缺口记录与三处差分结论 ([9a9a9fd](https://github.com/sekfung/ESCode/commit/9a9a9fdbffbc286fbb09520968f99632887a79d9))
  * 表格：startSavedWorkflow 转「已实现，差分一致」；setAssistantFeedback /
  * 新增 forkAssistant 子会话身份与边界、session/read 的 session 投影两节，含已知差异
  * startSavedWorkflow 一节记三方所有权、三段时序图与拒绝词表。

* **rust:** 记录 2026-10-10 发布回退复验结果 ([ddbf0e9](https://github.com/sekfung/ESCode/commit/ddbf0e96ab4ac2ab0fed036c9c5d680f6ed02d45))

* **rust:** 跨平台真机与发布回退验收记录；发布资产比对按 GitHub 命名规则归一 ([444d4a9](https://github.com/sekfung/ESCode/commit/444d4a932743bf1847798b573ff010949d92de21))


### Refactorings

* 产品更名为 ESCode ([13c5a1a](https://github.com/sekfung/ESCode/commit/13c5a1a2dd79cb4493e2a874f8c02665354465c9))
  * 包名、目录、crate、环境变量、协议名与深链 scheme 改为 ESCode
  * 图标、dmg 背景与应用内标识替换为 ESCode 标识
  * 上游 NOTICE 原文移至 third-party/upstream-zcode/，新 NOTICE 声明来源与修改
  * 保留外部服务主机名、插件市场 id 与旧插件清单目录回退（docs/specs/escode-rebrand.md）
  * 修复更名引入的测试：About 标识 viewBox、官方市场展示名（不再暴露 zcode-plugins-official
  * 补齐批量替换漏掉的旧名写法：Z Code（退出确认文案、X-Title 客户端标识、文档标题）与
  * 验证：typecheck / lint / architecture:check / 30 个漂移生成器 --check / cargo test（319 通过）

* **rust:** 拆出 shell 提醒模块，满足单文件 400 行边界 ([1bc0321](https://github.com/sekfung/ESCode/commit/1bc0321b1ce1d2b79572ab3b439afb7be7e13dfc))
