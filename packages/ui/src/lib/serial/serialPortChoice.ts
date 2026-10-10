import type { SerialPortInfo, SerialSessionSummary } from "@zcode/services";

export interface SerialPortOption {
  path: string;
  manufacturer?: string;
  /** 已有会话（含其他标签打开的、断开等待重连的）：选择即挂回，不重新打开。 */
  running: boolean;
}

/** 下拉选项：本机串口在前，其后补上已拔出但仍有会话（如等待重连）的串口。 */
export function buildSerialPortOptions(
  ports: readonly SerialPortInfo[],
  sessions: readonly SerialSessionSummary[],
): SerialPortOption[] {
  const running = new Set(sessions.map((session) => session.path));
  const options: SerialPortOption[] = ports.map((port) => ({
    path: port.path,
    ...(port.manufacturer ? { manufacturer: port.manufacturer } : {}),
    running: running.has(port.path),
  }));
  const listed = new Set(options.map((option) => option.path));
  for (const session of sessions) {
    if (!listed.has(session.path)) options.push({ path: session.path, running: true });
  }
  return options;
}

/**
 * 新标签的预选（只预选，不自动连接）：已绑定的 path 优先；否则上次用过且未被占用的串口；
 * 否则第一个空闲串口；都被占用时退回第一个串口（选择后即挂回该会话）。
 */
export function pickInitialSerialPath(input: {
  tabPath?: string;
  lastPath?: string;
  ports: readonly Pick<SerialPortInfo, "path">[];
  sessions: readonly SerialSessionSummary[];
}): string | undefined {
  if (input.tabPath) return input.tabPath;
  const running = new Set(input.sessions.map((session) => session.path));
  if (input.lastPath && !running.has(input.lastPath)) return input.lastPath;
  return input.ports.find((port) => !running.has(port.path))?.path ?? input.ports[0]?.path;
}
