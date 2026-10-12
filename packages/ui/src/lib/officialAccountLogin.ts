/**
 * 官方账号登录（zcode / Z.ai / BigModel OAuth）门禁策略。
 *
 * 当前发行版免登录：冷启动不再因为“没有账号 / 没有可用模型 Provider / 官方 JWT 失效”
 * 把用户强制带到官方登录入口，Root 直接进入主界面；未配置模型时由首页
 * `modelConfigMissing` 横幅引导配置。产品规则和自动入口清单见 docs/ui/login-page.md。
 *
 * 全部自动入口都读取本开关：provider 可用性启动守卫、
 * `useRootOAuthEffects` 的会话失效提示与官方 JWT 失效重启、
 * 以及退出登录后运行域被清空时的引导（`Root.openAutomaticLoginEntry`）。
 * 用户显式发起的入口（手动登录、模型供应商连接）不读取本开关。
 *
 * 官方登录相关代码（WelcomeScreen、LoginPanel、OAuth 服务、凭据存储、手动登录入口、
 * OAuth 回调链路、退出登录）全部保留，等待接入 Logto 第三方 IDP。接入后应让本策略
 * 由 Logto 会话事实驱动，而不是恢复写死的官方登录强制：把开关改回 true，
 * 上述自动入口就会恢复原行为。
 */
export const OFFICIAL_ACCOUNT_LOGIN_ENABLED: boolean = false;

/** 账号维度不可用（未登录 / 会话失效 / 官方 JWT 失效）时，是否允许强制打开官方登录入口。 */
export function isOfficialAccountLoginRequired(): boolean {
  return OFFICIAL_ACCOUNT_LOGIN_ENABLED;
}
