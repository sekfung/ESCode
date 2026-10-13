import { circuitIrSchema, type CircuitIR } from "@escode/shared/circuit";
import { CIRCUIT_EXPORT_SUFFIX } from "@escode/shared/circuit";

/**
 * 面板入口的文件分类与导出快照解析（规格 §8）。
 * 入口只有三类：KiCad 原理图/PCB（让 hook 直接请求确定性解析）、导出快照（只读展示）、
 * 其余文件不受支持。
 */

export type CircuitPanelFileKind = "kicad_sch" | "kicad_pcb" | "snapshot" | "unsupported";

export function circuitPanelFileKind(path: string | undefined | null): CircuitPanelFileKind {
  if (!path) return "unsupported";
  const lower = path.toLowerCase();
  if (lower.endsWith(CIRCUIT_EXPORT_SUFFIX)) return "snapshot";
  if (lower.endsWith(".kicad_sch")) return "kicad_sch";
  if (lower.endsWith(".kicad_pcb")) return "kicad_pcb";
  return "unsupported";
}

/** 导出快照内容（app/circuitExport.ts 写入的形状；`derived: true` 标记派生产物）。 */
export const circuitSnapshotSchema = circuitIrSchema;

export type CircuitSnapshotParseResult =
  | { ok: true; ir: CircuitIR }
  | { ok: false; reason: "unreadable" | "invalid" };

/**
 * 只读展示导出快照：服务永远不读回自己写出的 `.circuit.json`（§2 规则 8），
 * 因此这里在 renderer 侧按 IR schema 解析，不进服务、也不作为可写 IR。
 */
export function parseCircuitSnapshot(text: string | null | undefined): CircuitSnapshotParseResult {
  if (text === null || text === undefined) return { ok: false, reason: "unreadable" };
  try {
    const parsed: unknown = JSON.parse(text);
    const ir = circuitIrSchema.safeParse((parsed as { ir?: unknown } | null)?.ir);
    return ir.success ? { ok: true, ir: ir.data } : { ok: false, reason: "invalid" };
  } catch {
    return { ok: false, reason: "invalid" };
  }
}

export interface CircuitPanelError {
  code?: string;
  message: string;
}

/** 服务错误码 → 面板展示用的错误对象；Code 缺失时按 io_error 处理。 */
export function describeCircuitError(error: unknown): CircuitPanelError {
  if (error && typeof error === "object") {
    const code = (error as { code?: unknown }).code;
    const message = (error as { message?: unknown }).message;
    return {
      ...(typeof code === "string" ? { code } : {}),
      message: typeof message === "string" ? message : String(error),
    };
  }
  return { message: String(error) };
}

export function isStaleRevisionError(error: unknown): boolean {
  return describeCircuitError(error).code === "stale_revision";
}

export interface CircuitPanelRevisionMark {
  irId: string;
  revision: number;
  stale: boolean;
}

/**
 * 面板是否要按事件刷新（§3.3、§3.4，与 CONTRACT.md 对这两节的口径一致）：
 * 只处理同一 irId 的事件；`revision` 前进要刷新，`revision` 不变但 `stale` 翻转也要刷新
 * （源文件变化不涨 revision，只把 stale 翻成 true）。落后或重复的事件一律忽略，
 * 不能靠超时或全量重读掩盖顺序问题。
 */
export function shouldRefreshCircuitPanel(
  known: CircuitPanelRevisionMark | null,
  event: CircuitPanelRevisionMark,
): boolean {
  if (!known || known.irId !== event.irId) return false;
  return event.revision > known.revision || event.stale !== known.stale;
}
