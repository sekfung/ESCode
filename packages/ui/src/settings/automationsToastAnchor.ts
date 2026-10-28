/**
 * Automations 主区的 toast 锚点由 WorkspaceShellLayout 挂载、AutomationsSection 内部使用。
 * 常量单独成文件，避免壳层为了一个字符串静态引入整份 AutomationsSection（首屏不需要）。
 */
export const AUTOMATIONS_TOAST_ANCHOR_ID = "automations-main-toast-anchor";
