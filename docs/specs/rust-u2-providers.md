# Rust 阶段 2：`providers` 域设计（rust-unified-host.md 子 spec，优先序 6）

> 状态：P1（`model-selection.getView`）、P2（`provider-settings.getView/refresh`）、P3（Personal 写入面 10 法 + `resolveModelConfig`）、P4（`onDidChange`）已交付 · 2026-10-15；`testModelConnectivity` 已交付（§10）；P5a/P5b 与 Endpoint 动态解析已交付（§11/§12）；`provider-provisioning-target.apply` 登记为非缺口（§13）；P6 待做。决策按 §4 的推荐执行（用户 2026-10-15 确认）。
> TS 真相源：`packages/provider/src/**`（4829 行，领域逻辑）、`packages/provider-node/src/**`（1895 行，文件/锁/下载）、
> `packages/services/src/model-provider/**`（5303 行，Facade 服务、账号、套餐可用性）、`packages/shared/src/{model-config,model-selection*}.ts`。

## 1. 涉及的 channel（23 个成员，均未实现）

| channel                           | 成员数                        | 依赖的核心能力                                                                                   |
| --------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------ |
| `model-selection`                 | 2（`getView`、`onDidChange`） | Registry（已解析的 Provider/Model）+ 默认选择 + 修订号                                           |
| `provider-settings`               | 15                            | 同上 + 设置视图投影（issues、分层配置）+ 个人配置写入（Personal 仓库、锁、CAS 修订）+ 连通性测试 |
| `credential`                      | 3                             | 凭据加密存储（`ESCODE_CREDENTIAL_SECRET`）                                                       |
| `client-config` / `client-scenes` | 1 / 1                         | ApiClient（端点 origin + 认证头）+ 缓存                                                          |
| `provider-provisioning-target`    | 1                             | 个人配置写入                                                                                     |

> **已删除（2026-10-15，用户决定）**：整个套餐族——`oauth` / `coding-plan-subscription` / `highspeed-card` / `off-peak-task`（13 / 33 / 3 / 11 成员）、
> `config/provider/escode-builtin.json` 里的 10 个 `account:` 套餐 Provider 与账号鉴权全部移除，不再纳入迁移范围（依据见 `escode-rebrand.md`
> 「不保留的 Z.ai 专属契约」）。本 spec 的 P6 只余 `credential` / `client-config` / `client-scenes`。

## 2. 关键事实（读 TS 得到，非推测）

1. **配置文件两份，格式固定**（已被 Rust CLI `model` crate 读取）：
   - Built-in Release：`{schemaVersion:1, revision:int, config:{providerConfigRules:{templateRules,providerRules}, modelConfigRules:{modelRules,modelApiRules,providerSiteRules,templateModelRules,builtinProviderModelRules}}}`，严格 schema，禁含 `builtin:zapi`。
   - Personal：`{schemaVersion:1, config:{providerOrder?, providerConfigRules:{providerRules}, modelConfigRules:{providerModelRules,manualProviderModelRules}, defaultModelSelection?}}`。
2. **Built-in 来源不是单个文件**：Bundled（随包）+ Active/LKG（按 `平台/应用版本/Endpoint 哈希` 隔离的缓存，文件锁保护）+ Remote 同步器（下载、校验、`revision` 单调）；Active 路径由 TS 进程在运行时解析，Main 无法独立算出（审计 A16b 的根因）。
   2a. **Personal 读取有写副作用**：文件内容与规范编码不同（键序、遗留格式）时，`read()` 会加锁重写；文件无效时原样保留并以内存空配置降级（不备份不覆盖）；1 s 轮询变更并发 `onDidChange`。
3. **Registry 解析**（`resolver.ts`，376 行）：Account 叠加（先去 `group`）→ 模板叠加 → Personal 叠加 → 模型规则按 `[builtin 全部层..., personal 精确规则]` 顺序叠加；规则含 JS 正则（`^(?:pattern)$`，模型名 `i`）、URL 规范化。Provider/Model 的“完整性”由 zod 完整 schema 判定（含 `compileModelOptionMap` 对 `map` DSL 的编译）。
4. **默认选择**：`defaultModelSelection`（Personal 字段）→ 可选则用，否则 Registry 顺序里首个可见 Provider 的首个模型取**最高推理档**；`ModelSelectionView.revision` 单调递增（Registry 替换 + 默认选择变更各自推进）。
5. **Rust CLI 已有一份面向执行的紧凑实现**（`escode-cli-model` 的 `registry_rules.rs`/`registry_config.rs`，Value 驱动，读同一对文件，输出 picker 目录/`ModelPort`）。它不产出 Settings 视图（分层配置、issues、`executable/selectable`），也不做写入。

## 3. 切片（每片都以 TS 为 oracle 做差分，先 spec 再码）

| 片     | 内容                                                                                                                                                                                        | 验收                                                                                                                          |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| **P1** | 只读 Registry：读 Built-in（Bundled 文件，不含 Active/Remote）+ Personal，Value 驱动解析，产出 `model-selection.getView`（含 `preferredSelection`、`revision`）；`onDidChange` 先登记为缺口 | 同一对文件喂 TS `ProviderRuntime` 与 Rust，视图 `toEqual`；多组夹具（模板/账号覆盖/隐藏/禁用/无效模型/手动规则/默认选择失效） |
| **P2** | `provider-settings.getView`：分层配置投影 + zod 同文案的 `issues`（需要完整 schema 校验与错误文案对齐）                                                                                     | 同上，`issues` 逐条相等                                                                                                       |
| **P3** | Personal 写入面（15 个方法中的 10 个）：文件锁、CAS 修订、规范编码、`settings:*` 刷新；`provider-provisioning-target.apply`                                                                 | 与 TS 对同一初始文件执行同一操作序列，落盘字节相等                                                                            |
| **P4** | 变更通知：Personal 轮询、Built-in 文件监听、`onDidChange` 事件，修订推进语义                                                                                                                | 差分：外部改文件 → 两端事件序列一致                                                                                           |
| **P5** | Built-in 物化：Bundled/Active/LKG 选择、Endpoint 隔离路径、Remote 下载与 `revision` 单调；解开 **A16b**                                                                                     | 差分 + 注入式下载桩                                                                                                           |
| **P6** | `credential`、`client-config`、`client-scenes`（ApiClient 基础设施）                                                                                                                        | 各自 spec                                                                                                                     |

## 4. 待决策（需要产品/架构拍板，不应由实现者单方面定）

1. **Built-in 物化的所有者**：Rust Host 是否接管 P5（下载、Active/LKG、锁）？接管 → 统一 Host 目标达成，但与 TS Host 共存期间两个进程会争同一 Active 文件（需沿用同一把文件锁协议）；不接管 → Rust 只读 TS 物化好的 Active 文件，A16b 靠 Main 把路径告诉 Host，Rust Host 对 TS Host 有存活依赖。
2. **Personal 读取的写副作用**：Rust 的 `read` 是否也规范化重写？（TS 会；两进程同时读写同一文件时，不重写更安全，但与 TS 行为不同，需登记差异。）
3. **与 CLI `model` crate 的关系**：P1–P2 的解析逻辑是复用/扩展 `escode-cli-model` 的 Value 实现（单一实现，但把 Settings 视图需要的 issues/分层塞进执行 crate），还是在 `host-services` 里独立实现（两份解析，需差分长期守住一致）？

## 5. 风险

- zod 错误文案（`issues[].message`）要逐字对齐，只能靠差分语料覆盖；正则方言（JS 与 Rust `regex` 的回溯/前瞻差异）在 `modelMatch/apiTypeMatch/baseUrlMatch` 上是真实风险，Rust CLI 现用 `regex` crate，遇到前瞻/反向引用会整条规则失败（TS 可用）。
- `compileModelOptionMap` 是一门小 DSL（`@escode/model-option-map`），Rust 侧已有 `domain::option_map`；P1 必须确认两者对“合法/非法”的判定一致。

## 6. P1 交付记录与登记差异

**范围**：`model-selection.getView(input?)`（含 `preferredSelection`、`effectiveSelection`/`selectionIssue`、旧档位回溯）。配置路径由 Main 经
`rpcListen.runtimeEnv` 的 `ESCODE_BUILTIN_PROVIDER_CONFIG_FILE` / `ESCODE_PERSONAL_PROVIDER_CONFIG_FILE` 提供（缺失则整条 channel 回显式缺口，不按 HOME 猜）。
实现：`host-services/src/provider/`（`files` 文件解码、`overlay` 配置叠加、`rules` 模型规则、`complete` 完整性判定、`resolve` Registry 解析、
`selection` 选择与视图投影、`legacy` 旧档位回溯、`mod` 服务与指纹缓存）；`host-rpc/src/domains_provider.rs`。

**验收**：`packages/desktop/test/rustHostProvider.test.ts`——真实 `config/provider/escode-builtin.json` + 15 种 Personal 场景（无文件 / 空 / 多 Provider / 顺序 /
手动规则 / 智能规则禁用模型 / 默认选择的有效、未知 Provider、坏档位、缺档位、隐藏、旧档位 `off` 及其被个人覆盖阻断 / 非法文件降级 / 不支持版本）× 17 种 `getView` 入参
（含账号类 Provider 的分类），与真实 TS `ProviderRuntime.modelSelection` 逐项 `toEqual`。负向验证：旧档位回溯、最高档位推荐、Provider 顺序、账号分类各改坏一次，均失败
（手动规则的 `clearManual` 在合法输入下不可观察，由 `rules_tests.rs` 单测守住）。

**登记差异（不修）**：

1. **叶子值域校验粒度**：TS 对 Built-in / Personal 文件整体 zod 严格校验，叶子值非法则整份文件失败；Rust 只在“键集合 / 版本 / 唯一性 / 组”层面拒绝整份文件，叶子非法的单个 Provider/Model 由完整性判定排除出 Registry。
2. **正则方言**：规则里的 `modelMatch/apiTypeMatch/baseUrlMatch` 用 Rust `regex`（无前瞻/反向引用）；不支持的写法使该规则永不命中并 warn（TS 会按 JS 方言命中）。
3. **option map 判定**：TS 以 `compileModelOptionMap` 编译判定；Rust 用 `escode_cli_domain::option_map::evaluate` 以代表值试算（解析或试算失败即不合格）。
4. **URL 校验**：zod `url()` 对应 `url::Url::parse`（WHATWG），边缘写法可能不同。
5. **Personal 读取不规范化重写**（§4.2 决策）；变更检测基于“解析后再序列化”的内容指纹，而不是 TS 的规范编码 SHA-256。
6. **修订号**：首次加载为 1，之后每次内容指纹变化 +1；TS 在 Personal 变更时可能因默认选择源与 Registry 各发一次事件而 +2，具体数值不外露语义（只要求单调）。
7. **账号源**：P1 没有账号源，恒用 fail-closed Overlay（账号类 Provider 全部 `entitled:false`，且 `account-plan` 选择一律 `account-connection-unavailable`）。
   - 2026-10-15 补记：套餐族删除后 Built-in 已无 `account:` Provider，本条描述的分类/账号叠加路径在 Rust 侧成为死代码；待用户并行进行的 `builtin_source`（P5 物化）改动落定后一并清理（`selection.rs::classify`、`overlay` 的账号层）。
8. **Built-in 来源**：仅读 `ESCODE_BUILTIN_PROVIDER_CONFIG_FILE` 指向的一份文件（Bundled 或 TS 已物化的 Active），无 Active/LKG/Remote（P5）。
9. **`onDidChange`**：未实现（P4），事件订阅回 NotImplemented。

## 7. P2 交付记录

**范围**：`provider-settings.getView()` 与 `refresh(reason)`（Rust 版 refresh = 重读文件后返回最新视图；无写入）。视图含 `providerTemplates`、`providerOrder`、
`providers[]`（`enabled/executable/templateConfig/effectiveBuiltinConfig/personalConfig/effectiveConfig/issues/models[]`，模型含 `builtin/effectiveBuiltinConfig/
personalExactConfig/useRecommendedConfig/effectiveConfig/enabled/executable/selectable/issues`）。实现：`issues.rs`（完整性问题清单）、`settings_view.rs`（投影）、
`types.rs`（从 `resolve.rs` 拆出数据类型）、`resolve.rs`（现在同时产出全部已解析 Provider 与 Registry）。

**issues 的事实边界**（TS 实测）：能通过文件加载（稀疏 schema）的配置，完整性不合格只会是「必填叶子缺失（缺字段或 `null`）」、空白 `apiKey`、
Personal 暂存的非法/空白 `baseUrl`（`invalid-url`/`required-field-missing`）与 `missing-template`；其余值域错误（枚举、数值、option map 语法……）在 TS
里于加载阶段整份文件抛错，Rust 对此只做 P1 登记的“单个 Provider/Model 排除”，并给一条通用 `invalid-config`（无 TS 对应文案）。

**验收**：`rustHostProvider.test.ts` 在 P1 的 15 种 Personal 场景上追加 `provider-settings.getView` 与 `refresh` 的逐项 `toEqual`，并新增 4 类带问题的
Personal Provider（空白 key、非法 baseUrl、缺 api、缺 group、缺失模板）。负向验证：改 invalid-url 文案、关掉 group 缺失检查、翻转 `builtin`、
固定 `useRecommendedConfig` 均失败。

## 8. P3 交付记录（Personal 写入面）

**范围**：`provider-settings` 的 10 个写方法：`createPersonalProvider`、`savePersonalProviderOverlay`、`deletePersonalProvider`、`reorderPersonalProviders`、
`reorderPersonalModels`、`addPersonalModel`、`renamePersonalModel`、`deletePersonalModel`、`savePersonalModelDraft`、`setPersonalModelEnabled`。
另交付只读的 `resolveModelConfig`（`resolve_config.rs`：按当前规则合成；带 `personalConfig` 时把草稿当智能规则预览，`inheritedConfig` 只含 Built-in 规则，issues 复用 `model_issues`）。
`provider-provisioning-target.apply` 不在 Rust Host 范围，见 §13；`testModelConnectivity` 见 §10。

**所有者与事件顺序**（唯一写者 = Host 的 `ProviderService`；Personal 文件是唯一事实源）：

```
RPC 写方法
  → write_gate（Host 内全局串行；TS 是按 Provider 串行）
  → load()                      读最新 Registry 快照（指纹未变则复用）
  → Membership{inherited, personal_revision}   Host 算出，不接受 Renderer 自报
  → store::update               file_lock 目录锁（跨进程，与 TS 同协议）
        读最新文件 → transform(&mut Doc) → Doc::canonical()（encode→严格 decode→encode，越权/坏值落盘前失败）
        → write_private（0600 临时文件 + rename）
  → 使文件指纹闸门失效 → provider_settings_view()   重新加载并返回最新视图（revision 推进）
```

写入路径遇到不存在的文件按空配置（`providerOrder:[]`），遇到无效文件**报错而不覆盖**（读取路径仍按 P1 降级为空）。`Skip`（同名重命名）不加锁不写盘。
乐观并发：`membership.personal_revision` 必须等于锁内重读文档的 `Doc::revision()`（SHA-256(规范编码)），否则 `Provider Settings membership revision conflict`；
`savePersonalModelDraft` 另比 Registry `basedOnRevision`（`Provider Settings revision conflict`）与 Personal 版本（`Personal Provider Config revision conflict`）。

**模块**：`sparse.rs`（入参稀疏 schema）、`personal.rs`（`Doc`：编码/解码/精确规则增删改/revision）、`store.rs`（带锁读改写）、`mutations.rs`/`mutations_create.rs`/
`mutations_models.rs`（逐条对照 `config-service.ts`）、`service_write.rs`（`ProviderService` 写方法）、`host-rpc/src/domains_provider_write.rs`（RPC 面）。

**验收**：`rustHostProviderWrite.test.ts`——三组初始文件（带 Personal Provider/规则/默认选择的完整文件、无文件、脏成员+缺省 `providerOrder`）共 60+ 步操作，
每步对真实 TS `ProviderRuntime.providerSettings` 与 Rust 比较：成败、错误文案、返回视图（去修订号）、落盘文件语义（`toEqual`）。覆盖：启停/新增/重命名/删除/
调序 Model，创建（含模板、本地化名称、重名后缀、初始配置、非法入参），Overlay 保存（改名/重名/清名/组改写/账号 Provider 限制/元数据），Draft（原位编辑、重命名、
推荐空配置清规则、转手动、过期修订、内置模型限制），删除 Provider，以及 8 种 `resolveModelConfig`（继承/手动/无规则/账号/未知 Provider/草稿预览/改名预览/非法草稿）。负向验证：去掉新增模型的 `enabled:true` 叠加、去掉改名唯一性检查、跳过成员规范化、预览草稿的推荐标记——均被抓出（预览里的改名迁移因随后被 `setExact` 覆盖而不可观察，不单独断言）。
Rust 单测（`write_tests.rs`）：8 路并发创建不丢更新、坏文件不被覆盖、同名重命名不写盘、文件 0600 且无锁/临时残留、写后下一次视图可见、过期成员版本被拒。

**登记差异（不修）**：

1. **错误文案**：入参不满足稀疏 schema 时，TS 抛 zod issue JSON，Rust 抛 `路径: 原因` 一句话；其余业务错误文案逐字一致（差分断言）。
2. **`optionSpecs.*.map`**：写入校验只要求非空字符串，不做 `compileModelOptionMap` 编译检查。
3. **键序**：Rust 落盘键按字典序（`serde_json` Map），TS 按其构造序；两者互读后语义相同（TS 读到键序不同的文件会无害地重写一次）。
4. **`assertCurrent`**：TS 还比较“发布快照对象是否仍是当前快照”；Rust 以 `personal_revision` + 写闸门串行代替（Built-in 在请求中途换版本的窗口不单独拦截）。
5. **写闸门粒度**：全局串行，不是按 Provider；单 Provider 的可观察顺序一致。
6. **`legacy` 手动规则**：TS 读取时把旧完整手动规则收窄为可编辑叶子；Rust 读取不收窄，原样保留（写回时也不改写其它规则）。

## 9. P4 交付记录（变更通知）

**范围**：`model-selection.onDidChange` 与 `provider-settings.onDidChange`。事件负载是**事件发生时重新算出的视图**（TS 同：`facade.onDidChange(() => listener(getView()))`）。

**所有者与事件顺序**：

```
ProviderService::load()   ← 唯一的 Registry 替换点
   │ 内容指纹变化 → revision += 1 → changes.send_replace(revision)   （watch 通道，合并多次变化）
   ├─ 本进程写入后的 provider_settings_view()      同一路径
   └─ 轮询任务（首个订阅者出现时启动，1s，Weak<Service> 随服务消亡）    外部改文件
订阅者（每个 EventListen 一条流）：watch_changes() = 先 load 一次再登记接收端（初始快照不通知）
   → changed().await → 重建视图 → EventFire
```

指纹未变时轮询不读盘（P2 的 `FileStamp/ChangeGate`）；内容未变（touch/同内容重写）不推进修订号，不通知。视图重建失败只丢这一次通知并记 `eprintln`，下次变化再推。

**登记差异（不修）**：

1. **变更源**：TS 的 Personal 用 1s 轮询、Built-in 用文件监听 + Remote 刷新；Rust 的 Built-in 也走同一个 1s 指纹轮询（P5 引入 Active/Remote 后再评估监听）。
2. **model-selection 修订号**：TS 的 `ModelSelectionService` 自带单调计数（`max(自身, Registry)` 且每次事件 +1）；Rust 直接用 Registry 修订号——对 Renderer 只要求单调，不外露其它语义。
3. **加载失败时的轮询**：失败只重试，不向订阅者发错误事件（TS 有 `poll-error` 原因的内部事件，但视图层不转发）。

**验收**：`service_events_tests.rs`（初始快照不通知、外部改文件由轮询发现、同内容重写不通知、本进程写入通知）；`rustHostProviderEvents.test.ts`（真实 Host + RPC 客户端：
外部改文件后 `provider-settings` 与 `model-selection` 都收到事件，视图含新名称且修订号大于初始；本进程 `createPersonalProvider` 的返回视图与随后推送的事件同一修订）。
负向验证：去掉广播 → 外部事件等待超时失败。

## 10. `testModelConnectivity`

流程（TS `providerFacadeServices.ts` + `providerSettingsConnectivity.ts`）：等在途写入收口（写闸门释放）→ 从最新 Settings 视图按公共资格判定可测性
（provider 缺失/禁用 → `provider-unavailable`；model 缺失/禁用/有问题 → `model-unavailable`；provider 不可执行 → `provider-unavailable`；model 不可执行 → `model-unavailable`，
文案与 TS 逐字相同）→ 通过则把 `{providerId, modelId}` 作为 ModelSelection 交给该 workspace 的 runtime（`provider/testModelConnectivity`，120 s 超时），
成功回 `{success:true}`，失败回 `{success:false, error:{message}}`。

登记差异：TS 调用前会同步账号 Provider 配置（`ensureAccountProviderConfigSynced`），Rust 暂无账号源，跳过。
验收：`rustHostProviderWrite.test.ts` 追加 3 种资格失败与 TS 逐字比较；`domains_provider_connectivity_tests.rs` 覆盖资格链全部分支。
成功与失败路径已由 `rustHostCommitMessage.test.ts` 的本地假模型端点验证（runtime 经 `runtimeEnv` 读同一对 provider 文件）。

## 11. P5a：Built-in Active/LKG 物化（本地，无下载）

**决策（沿用 §4.1 推荐）**：Rust Host 接管物化；与 TS Host 共存期间沿用同一把 `<file>.lock` 目录锁协议（`escode_cli_host::file_lock`）。
P5a 只做本地部分；Remote 下载与控制文件租约拆为 P5b（需要 `client-config` / HTTP 客户端，属 P6 基础设施）。

**状态所有者**：Active 文件 = 缓存（可丢弃），所有者是持有目录锁的进程；Bundled = 可信基线，只读。Registry 只由 `ProviderService::load()` 替换。

**输入（`rpcListen.runtimeEnv`；P5a 版本，已被 §12 修订：只需 `ESCODE_BUILTIN_CACHE_ROOT` + `ESCODE_APP_VERSION` 即启用缓存，`ESCODE_ENDPOINT_ORIGIN` 改为可缺省的 env 基址，另有 `ESCODE_ENV`、`ESCODE_BUILTIN_REMOTE_REFRESH`）**：

| 变量                        | 含义（对应 TS `escodeBuiltinEnvironment`）         |
| --------------------------- | -------------------------------------------------- |
| `ESCODE_BUILTIN_CACHE_ROOT` | `environmentConfigRoot`（`resolveAppConfigDir()`） |
| `ESCODE_APP_VERSION`        | `appVersion`                                       |
| `ESCODE_ENDPOINT_ORIGIN`    | 当前控制面 Endpoint Origin（规范化后参与路径）     |

`platform` 由 Host 按 TS `resolveESCodeBuiltinClientPlatform` 同规则自算（`windows|darwin|linux` × `aarch64|x86_64`）。

**路径**：`<root>/runtime/provider/<platform>/<appVersion>/endpoint-<sha256(origin)[0..32]>/escode-builtin.json`；Origin 规范化 = 仅 `http(s)`，取 `URL.origin`；
`platform`/`appVersion` 不得为空、`.`、`..` 或含路径分隔符（否则整组配置视为无效，退回 Bundled，并 warn）。

**读取语义（锁内，与 TS `#readAndMaterializeLocked` 同）**：

1. 读 Bundled 与 Active，各自按 `parse_builtin` 校验；不存在 = 无候选，其它错误 = 带错误的无效候选。
2. 选择：二者有效且 `revision` 相同但内容不同 → Bundled（Active 失效）；否则取 `revision` 较大者；都无效 → 报错“Bundled 与 Active … 均不可用”。
3. 选中的内容与 Active 语义不同（规范 JSON 比较，不比字节）→ 私有原子写入 Active。
4. 目录锁/建目录/写入任一失败 → 绕过 Active，只用 Bundled（Active 是缓存，不能阻断基线）。

**验收**：`builtin_source_tests.rs`（路径与 TS 逐字相等的向量、选择矩阵、物化写入、损坏 Active 恢复、同 revision 冲突、锁失败降级）；
`rustHostBuiltinActive.test.ts`：同一 Bundled/Active 夹具喂 TS `NodeESCodeBuiltinProviderConfigSource` 与 Rust Host，`model-selection.getView` 逐项相等，
且 Rust 物化出的 Active 能被 TS 读回并解码为同一 Release；Endpoint 路径向量由 TS `resolveESCodeBuiltinCachePaths` 生成。

**登记差异（不修）**：Rust 写出的 Active 保留源文件的键序（TS 按自身 `toJSON` 序）；二者语义比较，不会互相改写。

**P5a 交付记录（2026-10-16）**：`provider/builtin_source.rs`（路径/选择/锁内物化）、`ProviderFiles::from_env`（三变量齐全才启用 Active）、
Main 侧 `buildRustHostProviderEnv`（`desktopRuntimeEnv.ts`）经 `rustHostManager` 的异步 `runtimeEnv` 下发（同时补齐 A16b 的 Bundled/Personal 路径下发）。
验收：`builtin_source_tests.rs` 12 项（含 TS 生成的 Endpoint 哈希向量）；`rustHostBuiltinActive.test.ts` 7 种 Active 场景下 `getView` 与 TS 全等、两端 Active 语义相同；
`rustHostProvider.test.ts` 在新二进制上仍通过；负向验证：同 revision 冲突改选 Active → 单测失败。
**验证**：套餐族删除提交后工作区 TS 测试恢复可跑；`rustHostBuiltinActive.test.ts` 与 `rustHostProvider.test.ts` 在真实工作区通过；`pnpm typecheck`、`pnpm lint`（0 error）、`architecture:check --changed` 通过。
**仍待做**：Endpoint 切换（需 Main↔Host 控制消息）；P5b（Remote 下载 + 刷新租约，依赖 `client-config`）；`ESCODE_TEST_*` 平台覆盖未对齐。

## 12. P5b：Remote 下载 + 刷新租约 + Endpoint 动态解析

**Endpoint 切换的所有者（修订 §11 的输入）**：Endpoint 是 `resolveESCodeEndpointOrigin({env, envBaseOrigin, overrideOrigin})` 的结果，其中 `overrideOrigin` 来自设置项
`escodeEndpointOrigin`——设置文件由 Rust Host 的 settings 域持有，Host 自己读即可，**不需要新的控制消息**。因此 §11 的 `ESCODE_ENDPOINT_ORIGIN` 改为“env 基址”（可缺省），新增 `ESCODE_ENV`：

| 条件                      | 结果（与 `shared/escodeEndpoint.ts` 逐条同）                                  |
| ------------------------- | ----------------------------------------------------------------------------- |
| `ESCODE_ENV=production`   | env 基址存在 → 规范化；否则默认 Origin（**忽略设置覆盖**）                    |
| 其它（含缺省，按 `test`） | 设置 `escodeEndpointOrigin` 非空 → 规范化；否则 env 基址；否则测试默认 Origin |

规范化 = §11 的 `normalize_endpoint_origin`（非法值使本次解析失败：Active 退回 Bundled、不下载，并 warn）。
每次 `load()` 重新解析；设置文件指纹加入指纹轮询，Endpoint 变化 → Active 路径变化 → 指纹变化 → 重载并推进修订号（TS `endpoint-changed` 同效）。启用缓存的条件变为：`ESCODE_BUILTIN_CACHE_ROOT` 与 `ESCODE_APP_VERSION` 都存在。

**下载（TS `downloadESCodeBuiltinRelease`）**：

1. `GET <origin>/api/v1/client/configs?app_version=&platform=` → `{code:0, data:{configs:{builtin_provider_config_json?: https URL（无 userinfo）}}}`；缺该字段 → `missing`（无 Release，非错误）。
2. `GET <该 URL>` → Release JSON，按 `parse_builtin` 校验（同 §11 的候选校验，含“禁含 builtin:zapi”）。
3. 不带凭据；总预算 20 s（含两次请求正文）；正文上限 10,000,000 字节；非 2xx → `HTTP <n>`；超时 → `timeout`；错误文案 `ESCode Built-in <stage>: <reason>`（stage = `client-config|cdn`），**不泄露 URL/query/正文**。
4. 出口走 `escode_cli_net`（`Purpose::WebFetch`，与 `system_probe` 同一单入口，代理/CA 规则由它统一）。

**刷新租约（TS `ESCodeBuiltinRemoteSynchronizer`，控制文件格式不变以便与 TS Host 共存）**：控制文件 `escode-builtin-refresh.json`（与 Active 同目录），锁内读-判-写，网络期间**不持锁**：

```
refresh(force):
  锁(control): 同 Endpoint 且 leaseUntil>now → skipped(lease-held)
               非 force 且同 Endpoint 且 nextEligibleAt>now → skipped(not-due)
               否则写 {leaseId, leaseUntil=now+30s, 换 Endpoint 则清 nextEligibleAt/failureCount}
  fetch（无锁）→ 复核 Endpoint 未变（变了 → finish(success) + skipped(endpoint-changed)）
  锁(active): apply — revision 小 → stale；相等且内容同 → unchanged，内容不同 → 错误（"相同 revision N 对应不同内容"）；大 → 写 Active → updated
  锁(control): finish — leaseId 仍是自己才写：成功 delay=1h/failure=0；失败 delay=min(60s·2^(n-1), 1h)/failure+1
```

**调度**：缓存启用且 `ESCODE_BUILTIN_REMOTE_REFRESH=1`（Main 显式声明；差分测试不带它，避免测试触网）时，服务在首次读取后启动 60 s 周期任务（`Weak<Service>`，随服务消亡），
成功改写 Active 即由指纹轮询推进修订号。刷新失败只记 `warn`（`ESCode Built-in Config 远端刷新失败`），不影响 Registry。

**验收**：`builtin_download_tests`（注入式 `Fetcher` 桩：缺字段=missing、非 https/带 userinfo 拒绝、HTTP 错误、超限、超时、Release 无效、zapi 拒绝、总预算跨两次请求）；
`builtin_sync_tests`（租约持有/未到期/强制、换 Endpoint 重置、stale/unchanged/冲突/updated、失败退避序列、finish 对 `leaseId` 的保护、`now` 注入）；
`endpoint_tests`（上表逐条 + 与 TS 向量对账）；`rustHostBuiltinRemote.test.ts`：本地 HTTP 桩服务器 + 真实 TS `ESCodeBuiltinRemoteSynchronizer` 与 Rust Host 分别对同一 Active/控制文件夹刷新，
Active 语义相同、控制文件字段相同（时间字段除外）、设置里切换 `escodeEndpointOrigin` 后 Host 的视图随之切到新 Endpoint 的缓存。
负向验证：去掉租约判定、放宽 revision 比较、忽略 `escodeEndpointOrigin` 覆盖，对应用例失败。

**登记差异（不修）**：① 出口为 Rust 统一出口，代理/CA 取自 Host 的网络配置而非 Main 的 `ApiClient`；② `redirect:error` 由统一客户端的 `redirect::Policy::none()` 达成：3xx 以 `HTTP 3xx` 报错（TS 报 fetch 的重定向错误），两者都失败且都不跟随；
③ 错误 stage 之外的 schema 细节统一为 `invalid response`（不复刻 zod 的 `invalid schema at <path>`）；④ 刷新结果的日志级别沿用 TS（`updated` 为 info，其余 debug，失败 warn）。

**§12 交付记录（2026-10-16）**：`provider/endpoint.rs`（Endpoint 动态解析，设置覆盖由 Host 读自己的设置文件）、`builtin_download.rs`（注入式 `JsonGet` + `HttpGet` 统一出口）、
`builtin_sync.rs`（租约/退避/`leaseId` 保护，控制文件格式与 TS 同）、`service_remote.rs`（60 s 调度）、`BuiltinSource` 的 Endpoint 动态路径与 `apply_remote_release`；
Main 侧 `buildRustHostProviderEnv` 改传 `ESCODE_ENV` + env 基址 + `ESCODE_BUILTIN_REMOTE_REFRESH=1`（不再传已解析的 Origin）。
验收：Rust 428 项全绿（endpoint 4、download 6、sync 11、source 12 项等）；`rustHostBuiltinRemote.test.ts`：本地 HTTP 桩 + 真实 TS 同步器，请求路径/query 相同、
控制文件字段相同、设置里切换 `escodeEndpointOrigin` 后 Host 视图与 TS 在同一 Endpoint 缓存上的视图相等；`rustHostBuiltinActive.test.ts` 加强为比较 `provider-settings` 视图
（真实 Built-in 没有 `providerRules`，此前“删第一条 Provider 规则”的变体是空操作；现改为删第一条模板规则，视图可区分）。
负向验证：忽略设置覆盖 → Remote 差分失败；同 revision 冲突改选 Active → Active 差分失败。
**测试基础设施修复**：`rustHostTestSupport.connectRpc` 原先等 `open` 之后才挂 `message` 监听，Host 的 `Initialize` 帧与升级响应同读时会丢，客户端永久不发请求；
Host 侧有并发工作（如本节的刷新）时必现。现在先建协议并订阅再等 `open`。
**未覆盖（已知）**：成功下载并写入 Active 的端到端（CDN URL 必须 https，本地桩无法提供）由 Rust 单测（注入桩）覆盖，TS 差分只覆盖 `missing` 路径与控制文件。

## 13. `provider-provisioning-target.apply`：登记为非缺口（2026-10-16，用户决定）

**事实（读 TS 得到）**：该 target 只在两种环境启用（`services/node.ts`：桌面附着的**远程**环境，或带 auth token 的独立 HTTP `server`），由 Window Host 经
`remoteProviderProvisioningService` 调用。本地窗口的 Rust Host 不会收到它。信封（`shared/provider-provisioning.ts`）的 `accountSettings`
（`providerFamilyDomain` / `providerFamilyConnectionSelections`）、`oauth-session` 凭据（`oauth:zai:*` / `oauth:bigmodel:*` / `escodejwttoken`）与 `accountProviderSource.refresh`
都属于 2026-10-15 已删除的账号/OAuth 套餐族。

**决定**：不在 Rust Host 实现。库存清单里该成员保持“未实现”，并在此登记原因而非当作缺口追踪。

**重启条件**：Rust Host 需要跑在远程环境（SSH/WSL/Docker）上时，按“只含 Personal 配置”的瘦身信封重新设计——`syncId` 幂等状态文件、Personal 文件锁内 CAS、失败回滚；
同时修改 shared 的 zod schema 去掉账号字段，并与 Window Host 的调用方对齐。届时另立 spec。
