import type { Event } from "@escode/rpc";
import {
  type CircuitBlock,
  type CircuitComponent,
  type CircuitCorrectionOp,
  type CircuitDiagnostic,
  type CircuitFirmwareDifference,
  type CircuitIrSummary,
  type CircuitNet,
  type CircuitQueryTraceResult,
  type CircuitSourceKind,
} from "@escode/shared/circuit";
import { ServiceChannels } from "@escode/shared";
import { createServiceDescriptor } from "../descriptors.js";
import type { CircuitCorrectionRecord } from "./domain/circuitCorrections.js";

export type { CircuitCorrectionRecord } from "./domain/circuitCorrections.js";

/**
 * CircuitService 公开契约（docs/circuit/README.md §3.1、§5；ADR 0005）。
 * 服务是 Circuit IR 的唯一所有者：解析、校验、修正、revision、持久化、源文件监听都在服务内完成；
 * Agent 走 broker、面板走 hooks，都只经这里的方法，不直接读写 IR 文件。
 */

/** 每个请求都携带 workspace 身份：本地用 workspacePath，远程 workspace 另带 identity 与 remoteSessionId。 */
export interface CircuitWorkspaceRef {
  workspacePath: string;
  workspaceIdentity?: string;
  remoteSessionId?: string;
}

export interface CircuitParseParams extends CircuitWorkspaceRef {
  /** workspace 相对路径（见 §4 source.path）。 */
  path: string;
  /** 已有 IR 且要重新解析时必须带当前 revision（§3.4）。 */
  baseRevision?: number;
}

export interface CircuitIrParams extends CircuitWorkspaceRef {
  irId: string;
}

/** 面板与 Agent 读取的 IR 视图：摘要 + 校验问题明细。 */
export interface CircuitIrView {
  summary: CircuitIrSummary;
  diagnostics: CircuitDiagnostic[];
  /** diagnostics 或 items 因响应上限被截断。 */
  truncated: boolean;
}

/** 解析 / 识别得到的 IR 中的元素查询结果（§5 circuit_query）。 */
export type CircuitQueryResult =
  | { by: "ref"; items: CircuitComponent[]; nextCursor?: string }
  | { by: "net"; items: CircuitNet[]; nextCursor?: string }
  | { by: "block"; items: CircuitBlock[]; nextCursor?: string }
  | { by: "trace"; items: CircuitQueryTraceResult[]; nextCursor?: string };

export interface CircuitQueryParams extends CircuitIrParams {
  by: "ref" | "net" | "block" | "trace";
  ref?: string;
  netId?: string;
  netName?: string;
  blockId?: string;
  blockName?: string;
  componentId?: string;
  pin?: string;
  cursor?: string;
}

export interface CircuitCorrectParams extends CircuitIrParams {
  baseRevision: number;
  ops: CircuitCorrectionOp[];
  author: { kind: "user" | "agent"; sessionId?: string };
}

/** `circuit_export`：把当前 IR 快照写入 workspace（§2 规则 8、§5）。 */
export interface CircuitExportParams extends CircuitIrParams {
  /** workspace 相对路径，必须以 `.circuit.json` 结尾（§8 的导出快照）。 */
  path: string;
}

export interface CircuitExportResult {
  path: string;
}

/** `circuit_compare_firmware` 的参数（§5、§7）：files 是 workspace 相对路径列表。 */
export interface CircuitFirmwareCompareParams extends CircuitIrParams {
  files: string[];
  cursor?: string;
}

export interface CircuitFirmwareCompareResult {
  items: CircuitFirmwareDifference[];
  nextCursor?: string;
}

/**
 * 渲染几何（§8「KiCad 根据解析出的几何信息自己渲染 SVG」）。
 * 这是解析产物的只读派生视图，按 revision 与 IR 对齐，不进 IR、不参与修正：
 * 面板只读它绘制 SVG，选中/高亮的元素通过 elementId 回到 IR。
 */
export interface CircuitLayoutShape {
  kind: "rectangle" | "polyline" | "circle" | "arc";
  /** 已变换到图纸坐标（mm）的点：矩形取对角两点，圆取圆心 + 半径点，弧取起/中/止。 */
  points: { x: number; y: number }[];
  filled: boolean;
}

export interface CircuitLayoutPin {
  number: string;
  name?: string;
  /** 引脚连接点（图纸坐标，与网络推导用的点一致）。 */
  connection: { x: number; y: number };
  /** 引脚画线的另一端（符号体内侧）。 */
  body: { x: number; y: number };
}

export interface CircuitLayoutText {
  kind: "reference" | "value" | "property";
  text: string;
  x: number;
  y: number;
  rotation: number;
  hidden?: boolean;
}

export interface CircuitLayoutSymbol {
  /** 对应 IR components[].id；电源符号等不属于器件的符号没有该字段。 */
  elementId?: string;
  libId: string;
  unit: number;
  x: number;
  y: number;
  rotation: number;
  mirror?: "x" | "y";
  /** 电源符号的网络名（Value），面板用来画电源标记。 */
  powerName?: string;
  shapes: CircuitLayoutShape[];
  pins: CircuitLayoutPin[];
  texts: CircuitLayoutText[];
}

/** 一张图纸（根图纸 path 为空串，子图纸为 `/<sheetUuid>` 链）。 */
export interface CircuitLayoutSheet {
  path: string;
  name?: string;
  /** 图纸尺寸（mm）：原理图取纸张，`.kicad_pcb` 取板框。 */
  page: { width: number; height: number };
  symbols: CircuitLayoutSymbol[];
  wires: { points: { x: number; y: number }[] }[];
  junctions: { x: number; y: number }[];
  labels: {
    text: string;
    x: number;
    y: number;
    rotation: number;
    kind: "local" | "global" | "hierarchical";
  }[];
}

export interface CircuitLayout {
  irId: string;
  revision: number;
  sheets: CircuitLayoutSheet[];
}

/** 面板列出修正记录时的一项（下标用于 dropCorrection）。 */
export interface CircuitCorrectionListEntry {
  index: number;
  record: CircuitCorrectionRecord;
}

/** 服务只读查询已有 IR（按源文件路径），用于面板打开文件时判断「还没有 IR」并拿到 baseRevision。 */
export interface CircuitFindBySourceParams extends CircuitWorkspaceRef {
  path: string;
}

export interface CircuitSourceInfo {
  kind: CircuitSourceKind;
  path: string;
  contentHash: string;
}

/**
 * 事件：写入完成（持久化之后）或源文件变化（stale 翻转，revision 不变）时发出（§3.3、§3.4）。
 * 同一 irId 的事件按 revision 单调递增；面板按 (revision, stale) 与本地状态比对决定是否刷新。
 */
export interface CircuitChangedEvent {
  irId: string;
  revision: number;
  stale: boolean;
}

export interface ICircuitService {
  /** 确定性解析（KiCad / 网表 / BOM）；已有 IR 时必须带 baseRevision（§3.4、§5）。 */
  parse(params: CircuitParseParams): Promise<CircuitIrView>;
  /** 按源文件路径查已有 IR；没有返回 undefined。面板用它判断「还没有 IR」并拿到 baseRevision。 */
  findBySource(
    params: CircuitFindBySourceParams,
  ): Promise<{ irId: string; revision: number; stale: boolean } | undefined>;
  get(params: CircuitIrParams): Promise<CircuitIrView>;
  /** 渲染几何；网表/BOM 这类没有几何的来源返回 undefined（面板显示语义视图）。 */
  getLayout(params: CircuitIrParams): Promise<CircuitLayout | undefined>;
  query(params: CircuitQueryParams): Promise<CircuitQueryResult>;
  /**
   * 固件比对（§7）：服务端确定性解析 devicetree / `.ioc` / C 头文件引脚宏，与 IR 的 MCU 引脚比对，
   * 输出四类差异；`file` 是 workspace 相对路径，`line` 尽量给出固件声明行号。
   */
  compareFirmware(params: CircuitFirmwareCompareParams): Promise<CircuitFirmwareCompareResult>;
  /**
   * 应用修正。baseRevision 与当前 revision 不一致时抛 `stale_revision`（调用方重读后重试）；
   * 不覆盖、不合并（§3.3）。新元素 id 由服务端生成。
   */
  correct(params: CircuitCorrectParams): Promise<CircuitIrView>;
  /**
   * 修正记录（corrections.json）：orphaned=true 的是重新解析后套用不上的修正，
   * 面板列出后由用户确认或删除（§8）。删除走 dropCorrection。
   */
  listCorrections(params: CircuitIrParams): Promise<CircuitCorrectionListEntry[]>;
  dropCorrection(
    params: CircuitIrParams & { baseRevision: number; index: number },
  ): Promise<CircuitIrView>;
  /**
   * 导出只读快照：写入 workspace 的派生产物，带派生产物标记；服务永远不读回它（§2 规则 8）。
   * 导出路径必须落在 workspace 内且以 `.circuit.json` 结尾。
   */
  exportSnapshot(params: CircuitExportParams): Promise<CircuitExportResult>;
  onChanged: Event<CircuitChangedEvent>;
}

export const ICircuitService = createServiceDescriptor<ICircuitService>(ServiceChannels.Circuit);
