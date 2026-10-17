# Rust 统一 Host 迁移：交接手册（2026-10-15）

> 面向**接手的 AI 会话**。读完这一页即可继续开工：目标、已完成边界、怎么验证、下一步从哪一行改起、以及踩过的坑。
> **2026-10-15 评审续写（以本段为准）**：过往实现已按源码逐项评审，结论与修复见
> `docs/reports/rust-unified-host-audit-2026-10-15.md`（A1–A15）。本轮新增：RPC 连接层（并发/取消/事件面）、
> Host 访问控制（进程密钥 + 一次性 capability + 拒绝 Origin）、runtime 反向请求应答、`escode-agent` V4 连接作用域
> （**对话主链路，dev 开关下已能跑一轮真实对话**，见 `docs/specs/rust-u2-agent-v4.md`）、契约清单改由生成器从 TS 源码
> 解析全集（51 channel / 619 成员）。下文旧段落中与此冲突处以本段与评审报告为准。
> **2026-10-15 续写**：§6.1（`getTaskSnapshot`）与 §6.2（`getTaskConfigOptions`）已落地，
> 详见 `docs/specs/rust-u2-tasks-index.md` §6.4；下一片建议从 §6.4.5 的「下一步（S2 收尾）」
> 即 `createSession`/`createTask` 继续（§6.3 事件面仍是最大件）。
> 权威文档（本手册不替代它们）：计划 `docs/specs/rust-unified-host.md`、质量报告 `docs/reports/rust-impl-quality-comparison-2026-10-14.md`、参考实现 https://github.com/MBearo/ZCode-rs 。
> 本手册的定位是"操作指南"：状态快照 + 固定动作 + 陷阱清单。**状态会过期，验证命令不会。**

---

## 0. 五分钟上手

```bash
# 1) 基线自检（AGENTS.md 要求）
node scripts/check-workspace-freshness.mjs

# 2) 读计划与子 spec（按顺序）
#    docs/specs/rust-unified-host.md           ← 总目标 + 阶段划分 + 验收
#    docs/specs/rust-u2-host-skeleton.md       ← Host 骨架/通路/域进度表
#    docs/specs/rust-u2-tasks-index.md         ← 会话与任务域（当前主战场，§6.x 是逐片进度）
#    docs/specs/rust-u1-runtime-lib.md         ← 阶段 1（已完成）的落地与欠账

# 3) 跑一遍门禁，确认接手时是绿的
cd apps/escode-host && cargo test --workspace           # 应全绿（本轮为 101 passed / 0 failed）
cd ../.. && pnpm check:escode-host                      # 边界 + 生成器 --check + fmt + clippy -D warnings

# 4) 看当前"缺口清单"（按 channel 统计已实现/待迁移；数据源 = 生成的 TS 全集 + 已实现表）
node scripts/generate-escode-host-service-inventory.mjs --check   # TS 服务面变了先重新生成
python3 - <<'PY'
import re, pathlib
ts = pathlib.Path("apps/escode-host/crates/host-proto/src/inventory_ts.rs").read_text()
done = set(re.findall(r'\("([^"]+)", "([^"]+)"\)', pathlib.Path("apps/escode-host/crates/host-proto/src/inventory_services.rs").read_text()))
for channel, members in re.findall(r'\("([^"]+)", &\[([^\]]*)\]\)', ts):
    names = re.findall(r'"([^"]+)"', members)
    implemented = [m for m in names if (channel, m) in done]
    print(f"[{channel}] implemented={len(implemented)} pending={len(names) - len(implemented)}")
PY
```

**当前战场**：阶段 2 优先序 5（会话与任务域）。S1（tasks-index 只读面）、S3a（侧栏状态写 + workspace 事件总线）、
S3b（syncer）、§6.4 快照读面与 `window-controller`（8/8）已完成；任务域剩余主件是 `createTask`/`createSession`
参数整形与 draft 注册表，其余见 §6 的新建议顺序。

---

## 1. 目标与边界（一句话版）

把桌面 App 的 Host 从 Node（Electron utility process）迁到 Rust：`apps/escode-host` 一个 App 级进程，向 Renderer 暴露与 TS 完全一致的 `@escode/rpc` 服务面；每个 workspace 一个运行时实例（默认 embedded，`runtime-link=child` 可切子进程）。**验收标准是"行为等价"，不是"能跑"**——所有新增行为都要有对照 TS 的证据（单测/差分/端到端），不可解释的差异一律显式登记。

阶段划分与验收见计划 §阶段 1–4；阶段 0（护栏/基线）与阶段 1（runtime 库化）已完成，阶段 2 进行中，阶段 3（切默认与清理）、阶段 4（四个超越项）未开始。

---

## 2. 代码地图（只看这些就够开工）

| 位置                                                                  | 作用                                                                                                                                                                                             |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `apps/escode-host/`                                                   | Host 独立 Cargo workspace：`crates/host-{core,proto,gateway,control,runtime,services,surface,rpc}` + 根 `escode-host` 二进制                                                                     |
| `apps/escode-host/crates/host-services/src/{settings,tasks_index}`    | 域存储层（settings 文件、tasks-index 读写）                                                                                                                                                      |
| `apps/escode-host/crates/host-rpc/src/`                               | Renderer 真实链路：`session.rs`（帧/分派/Channel 方法表）、`forward.rs`（运行时转发）、`domains*.rs`（各域方法表与实现）、`collation.rs`（ICU 排序）、`task_targets.rs`（任务→workspace 注册表） |
| `apps/escode-host/crates/host-proto/src/inventory.rs`                 | **契约清单**（channel/method + Implemented/Pending），与源码双向对账                                                                                                                             |
| `packages/services/src/escode-agent/escodeTaskServiceAdapter.ts`      | **TS 语义的真相源**（任务域 6.2k 行；改 Rust 前必读对应函数）                                                                                                                                    |
| `packages/services/src/session/taskIndexRepo.ts`                      | tasks-index 的 TS 读写语义（SQL/合并规则/排序）                                                                                                                                                  |
| `packages/desktop/src/main/{rustHostBridge,rustHostManager,index}.ts` | dev 开关 `ESCODE_RUST_HOST=1` 的 Main 侧接线（MessagePort ↔ RPC WebSocket 字节桥）                                                                                                               |
| `apps/escode-cli-rust/`                                               | 运行时（Engine）：`crates/core/src/app/*.rs` 是 legacy 方法实现（`session/*`）                                                                                                                   |
| `scripts/check-escode-host-boundaries.mjs`                            | Host 边界门禁（400 行、分层、**非测试代码 0 处 `std::env`**）                                                                                                                                    |
| `scripts/generate-escode-host-tasks-schema.mjs`                       | tasks-index 迁移账本/DDL 生成器（`--check` 已进 CI）                                                                                                                                             |

---

## 3. 已落地（按域，含证据入口）

| 域/主题                                                                                                                                                                                          | 状态                       | 证据                                                                                                         |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------- | ------------------------------------------------------------------------------------------------------------ |
| u2a 骨架：host-core（有界通道/原子写）、host-gateway（到达序/capability）、host-control（typed 控制消息）、host-runtime（实例表 single-flight/懒重启/收口残余）、host-surface（Renderer 传输面） | 完成                       | 各 crate 单测 + `crates/host-surface/tests/ws_surface.rs`                                                    |
| Renderer RPC 面（`@escode/rpc` 13 字节帧 + VQL + WebSocket + capability 一次性令牌 + 非回环 fail-closed）                                                                                        | 完成                       | `crates/host-rpc/tests/rpc_surface.rs`、`packages/desktop/test/rustHostRpc.test.ts`（真实 `@escode/client`） |
| dev 开关 `ESCODE_RUST_HOST=1`（App 级 Host + MessagePort 桥 + 失败回退 Node）                                                                                                                    | 完成                       | `packages/desktop/test/rustHostBridge.test.ts`（5 例）                                                       |
| **契约清单双向对账**（方法表即分派；Implemented 必须在表里、Pending 不得在表里、注册 channel ⊆ 清单）                                                                                            | 完成                       | `crates/host-rpc/tests/contract.rs`（3 例，含负向验证记录）                                                  |
| `setting` / `settings` / `system` 域                                                                                                                                                             | 完成                       | 双 Host 交替写读演练 `scripts/test-escode-host-interop.mjs`                                                  |
| `escode-session` 读/写/预热/展示面                                                                                                                                                               | 完成（不含 create/resume） | `crates/host-rpc/tests/rpc_session.rs`、真实客户端用例                                                       |
| `file` 域（stat/readdir/checkFilesExist/resolvePath/readFileRange/readTextFile/预览两法/工作区三法）                                                                                             | 完成                       | `crates/host-rpc/src/domains_file_tests.rs`、`tests/rpc_file.rs`、`rustHostRpc.test.ts`                      |
| `file.readdir` 的 ICU 排序（`localeCompare` 等价）                                                                                                                                               | 完成                       | `collation.rs` + 28 项语料 4 种 locale 差分                                                                  |
| **tasks-index 存储层**（迁移账本对账 + DDL 漂移哨兵 + `getTaskMeta` 投影 + 读写两套语义）                                                                                                        | 完成                       | `crates/host-services/src/tasks_index/*`（21 例）                                                            |
| `escode-task` 只读面：`getTaskMeta` / 四个列表分区 / `listTaskList`（全文搜索+分页+摘要窗口）/ `listGroupedTaskViewStructure`                                                                    | 完成                       | `tests/rpc_task.rs` + `rustHostRpc.test.ts` 的 Repo 差分（`toEqual` 全等 + 负向验证）                        |
| 任务目标注册表 + `getTaskModelSelection`                                                                                                                                                         | 完成                       | `task_targets.rs`（4 例）+ `rpc_task.rs`（2 例端到端）                                                       |
| 阶段 1（runtime 库化：`core-api::env`/`scope::Spawner`/`app-server::Link`/`embed`）                                                                                                              | 完成                       | `docs/specs/rust-u1-runtime-lib.md`、`docs/reports/rust-u1-runtime-library-2026-10-14.md`                    |
| P1 质量项 #5/#6/#7/#10/#11、#9 子项（net crate、RedactedString、FileStamp/ChangeGate、bash 策略表类型化、registry 类型化、SKILL.md 单点、插件错误类型化）                                        | 完成                       | 质量报告清单已勾选 + 各自单测                                                                                |

**未推送**：以上全部在本分支 `dev` 本地（截至本手册 `origin/dev..HEAD` 共 36 个提交，其中本工作流的阶段 0–2 提交为连续的一段）。**没有推送、没有开 PR**——接手前先确认是否要推送。

---

## 4. 固定动作（每次改动都这么做，别省）

1. **先改 spec 再改代码**（AGENTS.md）。域级新工作 → 该域子 spec 里先补"范围/所有者/接口/验收"，再写实现。
2. **一个切片一次提交**，切片 = 一个可独立验证的行为单元（例：一个服务方法族 / 一个存储语义 / 一个投影）。
3. **验收必须是对照 TS 的**，优先级：真实 TS 实现差分（`toEqual`）> 端到端（真进程/真 RPC）> 单测。只写"我实现了"不算。
4. **负向验证**：改错期望值/翻转过滤条件，确认测试真的会失败，再改回来。手册里所有"已验证"都做过这一步。
5. 门禁三连（都不能跳）：
   ```bash
   cd apps/escode-host && cargo fmt --all && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
   cd ../.. && pnpm check:escode-host          # 含边界（400 行/分层/0 处 std::env）+ 生成器 --check
   ```
   改了 TS / 需要真实客户端时再加：`npx vitest run --config vitest.config.ts packages/desktop/test/rustHostRpc.test.ts packages/desktop/test/rustHostBridge.test.ts`
6. **提交用显式路径**（见下一节的共享工作树警告）。
7. 差异要**显式登记**：写在对应子 spec 的"登记差异"里，并说明可见条件。禁止"用品近似"或"静默回退"。

### 4.1 加一个服务方法的固定形状（照抄这个模板）

迁移一个 channel method，改动点固定为**四处**（缺一处 CI 或运行时就会露馅）：

1. **方法表**（`crates/host-rpc/src/domains*.rs` 的 `Channel::new(...)`）：加 `("methodName", handler)`。
   表外方法由 `session::respond` 回显式缺口（错误信息里会自动列出已实现集合）——这正是"缺口显式可见"的实现方式。
2. **契约清单**（`crates/host-proto/src/inventory_services.rs` 的 `IMPLEMENTED`）：加 `(channel, member)`。
   全集来自生成资产 `inventory_ts.rs`（TS 源码解析），名字拼错会被 `implemented_entries_are_real_typescript_members` 拦下；
   不加 → `tests/contract.rs` 的 `source_method_table_matches_implemented_entries` 失败。事件（`onX`/`onDynamicX`）同表。
3. **实现**：纯路径类方法直接写（`domains_file*.rs` 的形态：`require_path` + `node_error` 文案 + `spawn_blocking_io`）；
   需要运行时的走 `forward.rs::runtime_call(instances, options, "session/xxx", params, Extract::Result|Field|FieldOr)`。
4. **测试**：单测（语义边界）+ 端到端（`tests/rpc_*.rs` 手写帧，看 `tests/support/mod.rs`）+ 能对差分就对差分（真实 TS 实现 `toEqual`）。

handler 的最小形状（注意：参数是**数组**，取第一个当对象；异步体在 `spawn_blocking_io` 里跑同步 IO）：

```rust
let my_method = handler(move |arg: Wire| {
    let params = first_object(arg);            // ProxyChannel.toService 打包的实参数组
    let context = context_ref(&context);       // 闭包是 Fn：句柄要克隆进去
    Box::pin(async move {
        let path = params["path"].as_str().unwrap_or_default().to_owned();
        context
            .run(move |index| index.my_call(&path).map_err(|e| e.to_string()))
            .await
            .map(Wire::Object)
    })
});
```

**自检**：改完先跑 `cargo test -p escode-host-rpc --test contract`（对账）、`cargo test --workspace`（行为）、
`pnpm check:escode-host`（门禁），再跑一次真实客户端套件（先 `cargo build`！）。

---

## 5. 陷阱清单（全部是实际踩过的）

### 5.1 仓库与环境

- **共享工作树**：本仓库同时有另一条工作流在改 `apps/escode-cli/packages/usb-plugin/`、`apps/usb-capture-helper/`、`docs/specs/usb-agent-tools.md`。**禁止 `git add -A` / `git add .`**（历史上曾把对方的 WIP 扫进提交）；只 add 自己的路径，`git status` 里非本工作流的改动一律不碰、不还原。
- **`cargo` 与差分套件不能并发**：差分套件用 `target/debug/escode-host`，并发 `cargo build` 会覆盖二进制导致随机失败。跑套件前先构建，跑完再构建。
- **vitest 套件用的是"已编译的 Host 二进制"**：改了 Rust 但没 `cargo build`，套件会报"does not implement xxx yet"这类**看起来像功能缺失**的失败。先 build。
- `pnpm fmt:check`（oxfmt）在**仓库范围内本来就是红的**（两千多个文件），只保证**你改的文件**干净；Host 侧的 fmt 由 `cargo fmt --check` 管。
- 生成资产（`tasks_index/schema.rs`）带 `#[rustfmt::skip]`：生成器输出必须与 `cargo fmt` 无关，否则 `--check` 会被格式化差异干扰。
- 生成资产的自校验哈希口径是**两端都能逐字节复现的规范文本**（`\t`/`\n` 连接），不是 `JSON.stringify`——两端 JSON 对转义的默认行为不同（踩过一次）。

### 5.1b 2026-10-15 新增陷阱

- **连 Host 要凭据**：`/ws` 与 `/ws/host` 都要一次性 capability，签发要进程密钥头 `x-escode-host-secret`
  （`rpcListening.secret`）；带 `Origin` 的请求一律 403。Rust 端到端用 `support::open(&surface)`，TS 用
  `rustHostTestSupport.ts` 的 `connectRpc(addr, secret)`。路径决定交付模式：`/ws/host` = desktop-continuous。
- **运行时错误名**是 TS 的 `ESCodeProtocolClientError`（带数字 `code`/`data`），不再是 `RuntimeError`；
  需要透传 `code` 的处理函数用 `session::handler_rpc`（`handler` 会退化成 `(name, message)`）。
- **本机设了 `HTTP_PROXY` 时**模型请求曾被送去代理（A15，已修）。mock 模型用例若卡在 `fault.provider.serverError`
  重试且 mock 计数为 0，先查代理。
- **帧泵在实例登记时就启动**（反向请求必须有人答）；首个订阅者前的帧进有界早期缓冲，不再靠「读循环随订阅启动」保留。
- **连接级 channel 工厂不得有副作用**：方法表探测（契约对账、server-info）会实例化工厂；后台任务放 `with_open` 钩子。

### 5.2 协议与运行时

- Renderer 的真实传输是 **`@escode/rpc` 跑在 WebSocket 上**（13 字节帧头 + VQL 长度 + DataType 标签 + `ProxyChannel.toService` 把 JS 实参打包成**数组**），不是 NDJSON。NDJSON 只用于 Host↔runtime 与 Host↔Main 控制通道。
- **运行时对可选字段严格拒绝 `null`**：构造转发请求时，缺省字段要**整个省略**，不能写 `null`（`forward.rs::runtime_target` 已按此实现，新方法照抄）。
- **响应形状**：`session/list` 的答案是 `{"id":..,"result":{"sessions":[…]}}`，读嵌套 `result`；`Extract` 枚举（`Result`/`Field`/`FieldOr`）就是为这个准备的。
- **RPC 错误帧是 JS Error 形状**：结构化 code 落在 `name`（不是 `code`），另有 `message`/`stack`。断言时别写错字段。
- ~~取消/注销等价 no-op、`EventListen` 显式缺口~~（2026-10-15 已实现：`host-rpc/src/connection.rs`；未知事件不回帧，与 TS 同）。
- 能力拒绝（401/404）前必须**先把请求头读完**再写响应，否则 TCP RST（踩过）。

### 5.3 测试夹具

- **tasks-index 夹具必须带迁移账本**：`TasksIndex::open` 会拒绝缺账本/多出未知迁移/checksum 不符的库（**设计如此**，别放宽）。建库用生成资产 `host_services::tasks_index::schema::{MIGRATED_DDL, EXPECTED_MIGRATIONS}`。
- **要创建会话就得给模型配置**：`session/create` 无配置报 `Selected model is unavailable`。用 CLI 测试同款夹具：`{providerId: "fixture", modelId: "fixture-model", reasoningLevel: "none", baseUrl: "http://127.0.0.1:9/v1"}` 写进 `model.json`，经 `RuntimeOptions.config`/`WorkspaceDescriptor.config` 传入。
- **别用不存在的 workspace 路径做夹具**：实例起不来，请求会挂到 30s 超时（看起来像"没实现"）。用真实临时目录。
- Host 侧单测的共享夹具在 `crates/host-services/src/tasks_index/test_support.rs`；RPC 端到端夹具在 `crates/host-rpc/tests/support/mod.rs`（`start` / `start_with_data_base_dir`）。

### 5.4 已登记的差异（**接手时不要丢，也不要私自"修好"**）

1. **搜索词大小写折叠**：Rust 用 `to_lowercase`（Unicode 默认映射），TS 用 `toLocaleLowerCase()`（默认 locale）。仅土耳其语/立陶宛语等特殊 case mapping 且 locale 命中时可见。
2. **`meta_json` 校验深度**：Rust 只做"必填 + 已声明字段的类型/枚举"浅层校验，不复制 zod 的深层对象校验。差异只在**损坏行**上可见（TS 走列回退，Rust 走 meta 投影）。
3. **file 域未覆盖 errno 的文案**：保留系统文案（TS 是 Node 文案）。客户端不解析该文案，仅展示。
4. **`getTaskModelSelection` 缺值**：Rust 回 `undefined`，TS 回 `null`（线上都表示"无值"）。
5. **`readWorkspacePresentation`**：TS 读取前会同步账号 provider 配置（providers/account 域未迁移）；Rust 按 Main 提供的配置解析。
6. **`escode-task` 的 `getTaskSnapshotBody`/`getTaskSnapshotRef`/`getTaskSnapshotToolCallsSlice`**：TS 适配器本身返回 `null`（未在该 Host 实现），Rust 可照抄这个语义。

---

## 6. 下一步（按建议顺序，含从哪一行改起）

> **2026-10-15 更新**：§6.1–6.3（快照读面、事件面）与 §6.4 的 `escode-session` 写面已落地。新的建议顺序：
>
> 0. **`window-controller` 本地子集**（8/8 已交付，`docs/specs/rust-u2-window-controller.md`）：订阅面 + `listTaskList` +
>    `mutateTask` + 归档删除，复用 tasks-index / syncer 会话摘要 / `WorkspaceEvents` 总线；TS oracle 帧差分 +
>    三处负向验证。**手机远控（dev 开关下已连 Rust Host）的时间线/置顶/归档/全局搜索不再空列表。**
>    远程 source 见该 spec §5-1 登记（属优先序 9）。
> 1. **`escode-task` 写面**（`createTask`/`sendPrompt`/`stopGeneration`/`respondPermission`/`setModel`/`setMode`/`closeTask`/`resumeTask`/`archiveWorkspaceTasks`）——
>    TS 适配器里这些走 `sendConversationCommandV4`（V4 面已就位，可复用 `agent_v4` 的 base 调用）。
>    **注意**：上述九法 Renderer 当前零调用（对话走 V4、侧栏走 reads + `window-controller`；`offPeakTaskStore.createTask`
>    调的是 `IOffPeakTaskService` 而非本 channel），消费方是 bots/off-peak 服务（属未迁移域）——价值在契约完整度与 S2 收尾
>    （`createTask` + draft 注册表）。**但 `escode-task` 里仍有 Renderer 活调用的未实现成员**（2026-10-15 评审按
>    `packages/ui/src` 实查）：分组视图写面 `createTaskGroup`/`renameTaskGroup`/`updateTaskGroupColor`/`deleteTaskGroup`/
>    `applyGroupedTaskViewOrder`（`useGroupedTaskView.ts`）、Claude 会话迁移 `scanImportableClaudeSessions`/`importClaudeSessions`、
>    `restartWorkspaceProcess`（`useWorkspaceSessionReload.ts`）、`getModelTrajectory`、`submitBotGroupInput`——它们在 dev 开关下
>    会直接报 NotImplemented，应先于上述九法排期（分组写面最优先：侧栏分组视图每次交互都会触发）；
> 2. **providers P2**（`provider-settings.getView`，`rust-u2-providers.md` §3）：模型/供应商设置面板的读面。
>    **它不解锁 A16b**（需要 P5：Built-in 物化，Rust 才能自己得出 Active 文件路径）；`git.generateCommitMessage` 需要 P1 的模型视图
>    **加上**运行时 `generateText`（A16b 的 env 注入），也不是 P2 能单独解开的；
> 3. **桌面窗口自身 host 的进程无关化**（`desktopHostProcess.ts:762`）→ dev 开关下跑桌面 E2E；
>    这是「全部桌面 E2E」这条验收的最后一环，受体量大（需 realtime/broadcast/cron 控制协议）见 §6.5（进程无关化）；
> 4. 其余 channel 按缺口清单（§0 第 4 步）逐域推进：~~git~~（已交付 17/18，见 `rust-u2-git.md`）/
>    ~~terminal~~（T1 已交付）/ ~~file-watcher~~（已交付）/ ~~window-controller~~（已交付）/ providers / extensions / automation / remote。

### 6.1 主件：`getTaskSnapshot`（escode-task，S2b）

为什么先做它：它是任务域写路径的总入口（resume → 快照 → 索引同步），也是手机 replayable 首屏读面；它的三个前置件（**注册表**、**索引写层**、**运行时转发**）都已就位。

TS 真相源（按调用顺序读）：

- `packages/services/src/escode-agent/escodeTaskServiceAdapter.ts`
  `getTaskSnapshot`（约 2876）→ `resumeSnapshotOrLegacy`（1340）→ `resumeSnapshot`（1175）→ `snapshotToESCode`（1629）→ `limitTaskSnapshotMessages`（3664）+ `buildSnapshotHistory`（3614）+ `buildSearchSnippets` 无关；
  `snapshotToMeta`（1408）及辅助：`deriveTitleFromSnapshot`（3599）、`fromESCodeMode`（3722）、`fromESCodeGoal`（4237）、`formatTaskMetaModelSelectionFromSnapshot`。
- 共享规则：
  - `packages/shared/src/escode-session-visible-content.ts`：`resolveESCodeVisibleSessionTitle` 229、`getESCodeUserVisibleMessages` 82、`isESCodeModelOnlySyntheticUserMessage` 50、`isESCodeCompactSummaryMessage` 67、goal 提醒文本前缀/标记常量 13–15。
  - `packages/shared/src/escode-session-task-status.ts`：`deriveESCodeTaskStatusFromSessionSnapshot` 57，含 `statusFromESCodeSession`、`hasBlockingActiveSnapshotRuntime`、`hasCompletedVisibleAssistantTurn`。
- 运行时段：`session/resume` 返回与 `session/read` **同形**的快照（`apps/escode-cli-rust/crates/core/src/app/session_resume.rs`）。
  - 快照键（实测）：`messages / projection / protocol / runtime / session / settings / slashCommands / todos`。
  - `session.{sessionId, traceId, title, createdAt, updatedAt, mode, model:{providerId,modelId}, status, sessionKind, workspace:{workspaceKey,workspacePath,workspaceIdentity?}}`。
  - `projection.{status, lastError?, target?, activeToolCalls, pendingPermissions, contextUsed, contextWindow, totalTokenCount, turnCount}`。
- 索引写：`syncTaskIndexSnapshot`（1504，**`subagent_child` 不写索引**）→ `syncTaskIndexMeta`（1499）→ 已实现好的 Rust `TasksIndex::sync_task_meta`（`crates/host-services/src/tasks_index/write.rs`）。

要点（都是可见行为，别简化）：

- `messageLimit` 语义：`normalizeSnapshotMessageLimit`（>0 才生效）+ `buildSnapshotHistory` 的 `{truncatedBefore: total > limit, totalMessages}`，消息取**尾部窗口**。
- `getTaskSnapshotWithEtag`：`sha256(JSON.stringify(snapshot))`；若 Rust 的键序与 TS 不同，etag 值会与 TS 不同（同一 Host 内自洽，客户端仍工作）。**要么逐字复刻键序，要么登记这条差异**。
- legacy 分支（`resumeSnapshotOrLegacy` 读到磁盘上的旧会话文档）依赖会话文件读取链，属独立工作包——先做 session 分支，legacy 分支登记为缺口。

### 6.2 `getTaskConfigOptions`（同族，做完成本很低）

`settingsToConfigOptions` → `packages/shared/src/escode-agent-model-state.ts:65`（`escodeSessionSettingsToESCodeConfigOptions`，约 120 行），依赖 `formatModelPickerValue`（`custom:provider:model` 编码）与 `getESCodeAgentModeSelectOptions`。前置件（注册表 + resume 转发）同一批。

### 6.3 事件面（`EventListen` / `EventDispose` → `ResponseType.EventFire`）

这是渲染器侧边栏/实时刷新的前提，也是质量报告点名的"投递层零测试"高风险面。

- TS 参考：`packages/rpc/src/channelClient.ts:149`（`requestEvent`）、`channelServer.ts:193`（`onEventListen`，事件以 `ResponseType.EventFire` 帧回推，客户端取 `response.data`）、`packages/rpc/src/protocol.ts`（RequestType/ResponseType 数值）。
- Rust 现状：`crates/host-rpc/src/session.rs:216` 目前直接回 NotImplemented；`Channel` 的 `listen` 面还不存在（需要按 (连接, channel, event) 维护订阅注册表 + dispose 语义 + 背压策略）。
- 与它同批的还有 workspace 事件源：任务的 `emitWorkspaceTaskListChanged`（TS 适配器）与 syncer 的 topic 帧摄入（`packages/services/src/escode-agent/escodeTaskIndexSyncer.ts`，1961 行，V4 topic：`sessionsIndexTopic`/`workspaceConfigTopic`）。

### 6.4 `createSession` / `createTask`（写路径主入口）

见子 spec `rust-u2-tasks-index.md` §5（参数整形四件事：可选省略、灰度字段只在命中时下发 true、`importedHistory` 不参与兼容省略、`persistence: "deferred"` 原样透传）与 §6 S2/S3。注意 TS 侧 `createSession` 还串了 MCP 解析（`withResolvedMcpServers`）、账号 provider 同步（`ensureAccountProviderConfigSynced`）与 task index syncer，**这些依赖项要先在子 spec 里写清是"一起做"还是"登记缺口"**。

### 6.5 其余排队项

- ~~`file` 域搜索/索引~~：已交付（`rust-u2-file-index.md`，`file` 17/17）。
- 优先序 4 剩余：无（`git`、`terminal`、`file-watcher` 已交付；`generateCommitMessage` 随 providers 域；terminal T2 = Alacritty/macOS 检测器，见 `rust-u2-terminal.md` §6.1）。
- 优先序 6–9：providers/account、extensions、automation、remote + runtime-link 选择器。
- **范围边界已核实（2026-10-17）**：`serial` / `plugin-ui-bridge` / `gen-ui` / `plugin-ui-app-tools` /
  `plugin-ui-sampling` / `cua-*` 共 43 个成员逐项读码定判——36 可迁（其中 `serial` 需重建 Rust 串口层、
  两个 `prepareSandbox` 需拆协议）、5 个 `cua-*` 不可迁、2 个 `cua-permission` 落点 Main。
  可迁待办由盘点的 310 修正为 **288**。依据与逐项证据见 `docs/specs/rust-u2-host-scope-boundary.md`。
- **快赢批已开工**：`usage-stats`(1)、`plugins`(7)、`git-checkpoint`(4)、`gen-ui`(5) 已交付——
  库存 228 → **251** 已实现 / 287 待办（`commands` 已 6/6 收口：目录解析 + `list` + 写面四法，
  见 `rust-u2-commands.md` §6/§7）；`subagents` 已 8/8 收口（list + 目录解析 + 状态写入面 + 写三法，见
  `rust-u2-subagents.md`）——库存 228 → **304** 已实现 / 234 待办；`serial` 已 14/14 收口（枚举/轮询、会话/自动重连、
  数据面、信号与循环，见 `rust-u2-serial.md` §8–§13）；`skill-sync` 5/5、`mcp-sync` 8/8、`settings-sync` 6/6 已交付（见 `rust-u2-sync.md` §1–§3）；`escode-task` 归档/日志/用量 4 个 + 生命周期 `createTask`/`resumeTask`/`closeTask` 与输入/停止/交互 5 个已迁（见 `rust-u2-tasks-index.md` §9–§11）（`usage-stats` 见 `rust-u2-misc-channels.md`、`plugins` 见
  `rust-u2-plugin-management.md` §8、`git-checkpoint` 见 `rust-u2-git.md` §8、`gen-ui` 见 `rust-u2-gen-ui.md`）。
- u2a 余项：**桌面窗口自身 host 的进程无关化**（`packages/desktop/src/main/desktopHostProcess.ts:762`）——需要 `windowHostProcessMap`/broadcastHub/taskRealtimeBus/`registerHostProcess` 接受"非 utility process 的 window host"；这是"dev 开关下跑通全部桌面 E2E"这条验收的最后一环。
- 阶段 3（切默认与清理，含 golden freeze）、阶段 4（零序列化进程内链路、跨实例共享缓存、启动/退出路径、严格等价测试网）未开始，见父计划。

### 6.6 质量报告里仍开着的项

`v4/command` schema 校验（P1 #10）、编辑匹配语料（edit-match fixture）、导入对照 corpus、hooks 投影单测、MCP 进程账本/遥测、rowsRange 尾页 SQL、`ModelStatus` 加厚、进程内 mock SSE 等——都在 `docs/reports/rust-impl-quality-comparison-2026-10-14.md` 的勾选清单里，任选其一都自成一片。

---

## 7. 名词与约定速查

- **channel/method**：Renderer 侧服务方法（`setting`/`system`/`file`/`escode-session`/`escode-task`…），与 TS `ServiceChannels` 同名；未迁移的方法必须回显式缺口（错误信息里列出已实现集合）。
- **方法表即分派**：`host-rpc` 里每个 channel 的 `Vec<(&str, MethodHandler)>` 就是分派来源，契约测试会与 `inventory.rs` 双向对账——**新增方法必须同时改两处**，否则 CI 失败（这是刻意设计）。
- **运行时 / 实例**：`Instances`（每 workspace 一个，身份键 = `workspaceIdentity?.trim() || workspacePath`）；`RuntimeLink::Embedded`（进程内）或 `Child`（`app-server --stdio` 子进程）。
- **embedded / child 双链路**：差分与回退用 child，默认 embedded；两者帧面相同。
- **desktop-continuous vs web-remote-replayable**：桌面实时链路与手机可恢复链路的语义不同（stream/snapshot/queue/重连），改一处要同时验两种。
- **Host 不读进程 env**：非测试代码 0 处 `std::env`（边界脚本强制），env 经 `core_api::env` 出口；业务事实（home/dataBaseDir/workspace 描述）一律由 Main 经控制消息提交。
