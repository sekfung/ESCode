# Rust 阶段 2：`providers` 域设计（rust-unified-host.md 子 spec，优先序 6）

> 状态：P1 已交付（`model-selection.getView`）· 2026-10-15；P2–P6 待做。决策按 §4 的推荐执行（用户 2026-10-15 确认）。
> TS 真相源：`packages/provider/src/**`（4829 行，领域逻辑）、`packages/provider-node/src/**`（1895 行，文件/锁/下载）、
> `packages/services/src/model-provider/**`（5303 行，Facade 服务、账号、套餐可用性）、`packages/shared/src/{model-config,model-selection*}.ts`。

## 1. 涉及的 channel（47 个成员，均未实现）

| channel                                                                   | 成员数                        | 依赖的核心能力                                                                                   |
| ------------------------------------------------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------ |
| `model-selection`                                                         | 2（`getView`、`onDidChange`） | Registry（已解析的 Provider/Model）+ 默认选择 + 修订号                                           |
| `provider-settings`                                                       | 15                            | 同上 + 设置视图投影（issues、分层配置）+ 个人配置写入（Personal 仓库、锁、CAS 修订）+ 连通性测试 |
| `credential`                                                              | 3                             | 凭据加密存储（`ESCODE_CREDENTIAL_SECRET`）                                                       |
| `client-config` / `client-scenes`                                         | 1 / 1                         | ApiClient（端点 origin + 认证头）+ 缓存                                                          |
| `provider-provisioning-target`                                            | 1                             | 个人配置写入                                                                                     |
| `oauth` / `coding-plan-subscription` / `highspeed-card` / `off-peak-task` | 13 / 33 / 3 / 11              | 账号连接、套餐、权益（账号域）                                                                   |

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
| **P6** | `credential`、`client-config`、`client-scenes`（ApiClient 基础设施）、账号/oauth/套餐/加速卡/闲时（按账号域另立 spec）                                                                      | 各自 spec                                                                                                                     |

## 4. 待决策（需要产品/架构拍板，不应由实现者单方面定）

1. **Built-in 物化的所有者**：Rust Host 是否接管 P5（下载、Active/LKG、锁）？接管 → 统一 Host 目标达成，但与 TS Host 共存期间两个进程会争同一 Active 文件（需沿用同一把文件锁协议）；不接管 → Rust 只读 TS 物化好的 Active 文件，A16b 靠 Main 把路径告诉 Host，Rust Host 对 TS Host 有存活依赖。
2. **Personal 读取的写副作用**：Rust 的 `read` 是否也规范化重写？（TS 会；两进程同时读写同一文件时，不重写更安全，但与 TS 行为不同，需登记差异。）
3. **与 CLI `model` crate 的关系**：P1–P2 的解析逻辑是复用/扩展 `escode-cli-model` 的 Value 实现（单一实现，但把 Settings 视图需要的 issues/分层塞进执行 crate），还是在 `host-services` 里独立实现（两份解析，需差分长期守住一致）？

## 5. 风险

- zod 错误文案（`issues[].message`）要逐字对齐，只能靠差分语料覆盖；正则方言（JS 与 Rust `regex` 的回溯/前瞻差异）在 `modelMatch/apiTypeMatch/baseUrlMatch` 上是真实风险，Rust CLI 现用 `regex` crate，遇到前瞻/反向引用会整条规则失败（TS 可用）。
- `compileModelOptionMap` 是一门小 DSL（`@escode/model-option-map`），Rust 侧已有 `domain::option_map`；P1 必须确认两者对“合法/非法”的判定一致。
- 账号相关（P6）牵涉登录态与网络，无法纯文件差分，需要桩化的 ApiClient。

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
8. **Built-in 来源**：仅读 `ESCODE_BUILTIN_PROVIDER_CONFIG_FILE` 指向的一份文件（Bundled 或 TS 已物化的 Active），无 Active/LKG/Remote（P5）。
9. **`onDidChange`**：未实现（P4），事件订阅回 NotImplemented。
