import {
  createUuid,
  SERIAL_QUICK_COMMANDS_MAX,
  serialQuickCommandSchema,
  type SerialQuickCommand,
} from "@escode/shared";
import { buildSerialSendPayload, type SerialBytesResult } from "@/lib/serial/serialFormat.js";

/**
 * 串口快捷指令的列表操作（docs/specs/serial-port-debugger-phase3.md 第 3 节）。
 * 列表存在全局设置 serialQuickCommands 中，这里只做纯变换，由面板调用设置更新写回。
 */
export type SerialQuickCommandDraft = Omit<SerialQuickCommand, "id">;

export function addSerialQuickCommand(
  list: readonly SerialQuickCommand[],
  draft: SerialQuickCommandDraft,
): SerialQuickCommand[] {
  if (list.length >= SERIAL_QUICK_COMMANDS_MAX) return [...list];
  return [...list, { ...draft, id: createUuid() }];
}

export function updateSerialQuickCommand(
  list: readonly SerialQuickCommand[],
  id: string,
  patch: Partial<SerialQuickCommandDraft>,
): SerialQuickCommand[] {
  return list.map((item) => (item.id === id ? { ...item, ...patch } : item));
}

export function removeSerialQuickCommand(
  list: readonly SerialQuickCommand[],
  id: string,
): SerialQuickCommand[] {
  return list.filter((item) => item.id !== id);
}

/** 拖拽排序：把 activeId 移到 overId 的位置。 */
export function moveSerialQuickCommand(
  list: readonly SerialQuickCommand[],
  activeId: string,
  overId: string,
): SerialQuickCommand[] {
  const from = list.findIndex((item) => item.id === activeId);
  const to = list.findIndex((item) => item.id === overId);
  if (from < 0 || to < 0 || from === to) return [...list];
  const next = [...list];
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved!);
  return next;
}

export interface SerialQuickCommandImportResult {
  commands: SerialQuickCommand[];
  imported: number;
  /** 校验失败被跳过的条目数。 */
  skipped: number;
  /** 超过上限被丢弃的条目数。 */
  dropped: number;
  error?: "invalidJson";
}

/** 导入为追加：每条重新分配 id，非法条目跳过，超过上限的部分丢弃。 */
export function importSerialQuickCommands(
  existing: readonly SerialQuickCommand[],
  json: string,
): SerialQuickCommandImportResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return { commands: [...existing], imported: 0, skipped: 0, dropped: 0, error: "invalidJson" };
  }
  if (!Array.isArray(parsed)) {
    return { commands: [...existing], imported: 0, skipped: 0, dropped: 0, error: "invalidJson" };
  }
  const valid: SerialQuickCommand[] = [];
  let skipped = 0;
  for (const entry of parsed) {
    const candidate = serialQuickCommandSchema.safeParse({
      ...(entry && typeof entry === "object" ? entry : {}),
      id: createUuid(),
    });
    if (candidate.success) valid.push(candidate.data);
    else skipped += 1;
  }
  const room = Math.max(0, SERIAL_QUICK_COMMANDS_MAX - existing.length);
  return {
    commands: [...existing, ...valid.slice(0, room)],
    imported: Math.min(valid.length, room),
    skipped,
    dropped: Math.max(0, valid.length - room),
  };
}

/** 导出不带 id（导入时重新分配），便于跨机器共享。 */
export function exportSerialQuickCommands(list: readonly SerialQuickCommand[]): string {
  return JSON.stringify(
    list.map(({ id: _id, ...rest }) => rest),
    null,
    2,
  );
}

/** 快捷指令的发送字节：与发送栏同一编码规则（文本加行尾、HEX 原样，校验和在内容之后、行尾之前）。 */
export function buildSerialQuickCommandPayload(command: SerialQuickCommand): SerialBytesResult {
  return buildSerialSendPayload({
    input: command.data,
    mode: command.mode,
    lineEnding: command.lineEnding,
    ...(command.checksum ? { checksum: command.checksum } : {}),
  });
}
