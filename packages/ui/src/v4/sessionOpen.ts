/** 会话打开时的冷/热/保活状态。 */
export type SessionOpenKind = "cold" | "warm" | "keep_warm";

/** 打开会话的入口；用于会话数据层区分恢复路径。 */
export type SessionOpenTrigger =
  | "sidebar"
  | "search"
  | "deeplink"
  | "reload"
  | "subagent"
  | "selection"
  | "split"
  | "pane";
