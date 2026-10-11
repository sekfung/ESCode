type Message = Record<string, any>;

/**
 * 输入命令未写 mode 时补上 harness 的默认协作模式（见 rust-permission-modes.md「集成测试约定」）。
 * 只补输入类命令；显式 mode（含测试刻意传入的非法值）原样保留。
 */
export function withDefaultMode(
  type: string,
  payload: Message,
  defaultMode: string | undefined,
): Message {
  if (!defaultMode) return payload;
  const fill = (input: Message) => ("mode" in input ? input : { ...input, mode: defaultMode });
  if (type === "sendText" || type === "sendGoalCommand") return fill(payload);
  if (type === "createSession" && payload.firstInput) {
    return { ...payload, firstInput: fill(payload.firstInput) };
  }
  return payload;
}
