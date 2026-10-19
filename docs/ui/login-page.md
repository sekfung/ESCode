# Login Page

## 当前发行版：免登录

本发行版不要求官方账号登录。冷启动不会因为“没有账号 / 没有可用模型供应商 / 官方 JWT 失效 / 退出登录后运行域被清空”把用户强制带到官方登录入口，`Root` 直接进入主界面；未配置模型时由首页 `modelConfigMissing` 横幅引导配置。

唯一开关是 `packages/ui/src/lib/officialAccountLogin.ts` 的 `OFFICIAL_ACCOUNT_LOGIN_ENABLED`（当前为 `false`）。它统一决定所有**自动**登录入口：

| 自动入口                      | 开关为 `false`（当前）                                           | 开关为 `true`（历史行为）                           |
| ----------------------------- | ---------------------------------------------------------------- | --------------------------------------------------- |
| 启动 provider 可用性守卫      | 不打开登录入口，直接进主界面                                     | 打开 `WelcomeScreen`（`startup-provider-required`） |
| 缓存会话失效（官方 JWT 过期） | 不弹“登录已过期”提示、不打开登录入口；仍清理失效凭据并清空登录态 | 弹提示并进入 `session-expired`                      |
| 官方 JWT 失效广播             | 只记日志，不重启、不打开登录入口                                 | 提示后重启应用重新登录                              |
| 退出登录后运行域被清空        | 停留在主界面，由 `modelConfigMissing` 横幅引导                   | 打开 `WelcomeScreen`（`logout-provider-required`）  |

用户**显式**发起的登录入口不受开关影响，仍然可用：

- 手动登录入口：`WelcomeScreen`（`manual-login`），从设置页或 workspace shell 的登录操作打开
- 模型供应商连接 / Rewards / Coding Plan 升级引导：`WelcomeScreen`（`provider-request`），打开后自动启动指定 provider 的 OAuth
- 登录内容容器：使用 `bg-background`、`border-popover-border`、`shadow-md`

官方登录相关实现（`WelcomeScreen`、`LoginPanel`、OAuth 服务、凭据存储、手动登录入口、`onOAuthCallback` 链路、退出登录）全部保留，等待接入 Logto 第三方 IDP。接入后本策略应由 Logto 会话事实驱动，而不是恢复写死的官方登录强制；把开关改回 `true` 即可恢复上表的历史行为。

“会话失效”的判定只影响提示与路由，不影响凭据事实：`oauthService.restoreCachedSessionState()` 仍然清理失效凭据，`setUser(null)` 仍然落定登录态，开关关闭时只是不再把结果升级为强制登录入口。

## 登录页内容

所有应用内登录入口都收敛到 `WelcomeScreen`，并复用 `LoginPanel` 的 OAuth 和 API Key 流程，避免手动登录、模型供应商连接出现多套状态机。

完成 OAuth 回调、保存 API Key 或点击 API Key 表单底部的“跳过”后，登录页关闭并回到正常 Root 路由。

用户点击“跳过”或从其它入口进入没有可用模型的空草稿首页时，首页必须展示已有的 `modelConfigMissing` 错误横幅，提供升级和模型配置入口。该横幅只投影本地 provider readiness，不触发 Agent 启动、deferred draft session 创建、联网探测或预热失败重试；保存合法 provider/model 并完成草稿模型水合后自动清除。

空草稿同时展示 workspace / Git context header 时，错误横幅必须作为独立 surface 位于输入卡外壳上方，不能嵌入包裹 context header 与编辑器的圆角卡片；横幅与输入卡之间固定保留 `mb-6`（24px）间距。视觉顺序固定为“错误横幅 → 独立间距 → 选择工作区 / Git 分支 → 输入框”。桌面端和手机 Web 端使用同一 DOM 顺序与容器层级；响应式布局只能改变换行和密度，不能把错误横幅移动到工作区入口下方或重新并入输入卡外壳。

API Key 登录表单在输入框为空时于右侧展示 `Get API Key` 外链入口；用户已经输入或回填 API Key 后隐藏该入口，避免链接占用密钥编辑空间。链接目标跟随当前选择的 provider：

- Z.ai：`https://z.ai/manage-apikey/apikey-list`
- BigModel：`https://bigmodel.cn/apikey/platform`

首次进入 API Key 登录表单时，provider 默认值只根据当前 App 已解析的 `locale` 决定：简体中文（`zh-CN`）默认选择 BigModel，英文（`en-US`）默认选择 Z.ai。`locale` 已包含“跟随系统”语言偏好的解析结果，因此表单不再单独读取系统语言或 provider 历史配置。该规则只负责表单初始化；用户手动切换 provider 后，不得因语言广播覆盖当前选择。

外链必须通过 `IPlatformService.openExternal()` 打开，保持桌面端 shell 和 Web 端 `window.open` 行为一致。输入框仍保留密码类型和回车提交能力，链接按钮只负责跳转获取密钥，不参与保存流程。

API Key 登录表单的取消按钮底部展示“跳过”链接按钮。跳过只写入当前选择 provider 对应的 `providerFamilyDomain`、`providerFamilyDomainUpdatedAt` 和 `providerFamilyDomainMigrated`，用于确认 Z.ai / BigModel 运行域；它不保存 API Key、不触发 API Key 登录成功事件，也不修改 `modelProviderFamilyModes`。

启动自动引导完成后有一个新用户直达路径：如果当前没有已打开 workspace，Root 使用 Main 注入的 conversation descriptor，必要时通过 `fileService.ensureConversationWorkspace()` 幂等确保 `<dataBaseDir>/.escode/workspace/default` 存在。该 workspace 提供真实 cwd，但 UI 保持“未选择项目”，用户无需经过项目选择页即可直接对话。该路径只在启动自动引导（`startup-provider-required`）打开过登录页时生效，免登录发行版下首次启动直接走同一条默认 workspace 兜底，不经过登录页。若用户跳过登录且仍没有可用模型，该草稿首页按上一条规则显示 `modelConfigMissing` 横幅。

手动登录和模型供应商连接完成后只关闭 `WelcomeScreen` 并回到原界面，不会创建默认 workspace。

## 验证

- `packages/ui/test/rootStartupGate.test.ts`：自动登录入口理由在开关关闭时被抑制。
- `packages/ui/test/providerAvailabilityLoginEntryGuard.test.ts`：开关关闭时守卫不打开登录入口。
- `packages/desktop/test/e2e/manual-review/pending/jwt-expiry-reauthentication.test.ts`：过期官方 JWT 冷启动直接进主界面、清理失效凭据、不弹登录过期提示。
- 执行 `pnpm typecheck`、`pnpm lint` 与 `pnpm test:unit`。
