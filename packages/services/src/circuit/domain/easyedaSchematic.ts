import { CircuitServiceError } from "./circuitErrors.js";
import { decodeEasyedaLog, type EasyedaDocument, type EasyedaRecord } from "./easyedaLog.js";

/**
 * EasyEDA V3 `.esch2` 原理图页的确定性提取（格式见 easyeda-api skill 的 `format/schematic/`）。
 *
 * 一份 `.esch2` 是若干文档的顺序拼接：`SYMBOL` 文档定义符号（PART 子库与带 NAME/NUMBER 的 PIN），
 * `SCH_PAGE` 文档放本页的 COMPONENT / WIRE / BUS 与它们的 ATTR。
 * 图元之间的从属关系只有 `ATTR.parentId` 一条线：`parentId == 器件 id` 是器件属性（Designator /
 * Value / Device / Footprint / Symbol），`parentId == 导线 id` 且 key 为 `NET` 是网络名；
 * 形如 `<器件 id><引脚 id>` 的复合 parentId 是引脚属性覆盖，不参与器件属性归并。
 * 纯函数、零 IO；结构不符的地方按 `unsupported_format` 报错，不猜。
 */

export interface EasyedaPin {
  number: string;
  name?: string;
  /** 符号坐标系里的连接点（远离符号体的那一端），单位 0.01 inch。 */
  x: number;
  y: number;
}

/** 符号文档里一个 PART 子库的引脚集合（单 Part 器件的 partId 为空串）。 */
export interface EasyedaSymbolPart {
  symbolUuid: string;
  partId: string;
  pins: EasyedaPin[];
}

export interface EasyedaComponent {
  id: string;
  /** 符号子库 ID；单 Part 器件为空串。 */
  partId: string;
  position: { x: number; y: number };
  rotation: number;
  isMirror: boolean;
  ref?: string;
  value?: string;
  partNumber?: string;
  footprint?: string;
  symbolUuid?: string;
}

export interface EasyedaWire {
  id: string;
  isBus: boolean;
  netName?: string;
  /** 折线点串：`dots` 的每个元素是一段连续折线的 X1 Y1 X2 Y2 … */
  segments: { x: number; y: number }[][];
}

export interface EasyedaSchematic {
  pageUuid: string;
  components: EasyedaComponent[];
  symbols: EasyedaSymbolPart[];
  wires: EasyedaWire[];
}

const COMPONENT_ATTR_KEYS = ["Designator", "Value", "Device", "Footprint", "Symbol"] as const;
type ComponentAttrKey = (typeof COMPONENT_ATTR_KEYS)[number];

function textOf(value: unknown): string | undefined {
  if (typeof value === "string") return value.length > 0 ? value : undefined;
  if (typeof value === "number") return String(value);
  return undefined;
}

function numberOf(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function attrsByParent(records: readonly EasyedaRecord[]): Map<string, EasyedaRecord[]> {
  const byParent = new Map<string, EasyedaRecord[]>();
  for (const record of records) {
    if (record.type !== "ATTR") continue;
    const parentId = textOf(record.payload["parentId"]);
    if (parentId === undefined) continue;
    const list = byParent.get(parentId);
    if (list) list.push(record);
    else byParent.set(parentId, [record]);
  }
  return byParent;
}

function attrValue(records: readonly EasyedaRecord[], key: string): string | undefined {
  for (const record of records)
    if (record.payload["key"] === key) {
      const value = textOf(record.payload["value"]);
      if (value !== undefined) return value;
    }
  return undefined;
}

function parsePoints(value: unknown): { x: number; y: number }[][] {
  if (!Array.isArray(value)) return [];
  const segments: { x: number; y: number }[][] = [];
  for (const raw of value) {
    if (!Array.isArray(raw)) continue;
    const numbers = raw.filter((entry): entry is number => typeof entry === "number");
    if (numbers.length < 4 || numbers.length % 2 !== 0) continue;
    const points: { x: number; y: number }[] = [];
    for (let index = 0; index < numbers.length; index += 2)
      points.push({ x: numbers[index]!, y: numbers[index + 1]! });
    segments.push(points);
  }
  return segments;
}

function parseComponent(records: readonly EasyedaRecord[], attrs: EasyedaRecord[]): EasyedaComponent | undefined {
  const record = records[0];
  if (record === undefined || record.id === undefined) return undefined;
  const positionX = numberOf(record.payload["positionX"]);
  const positionY = numberOf(record.payload["positionY"]);
  if (positionX === undefined || positionY === undefined) return undefined;
  const component: EasyedaComponent = {
    id: record.id,
    partId: textOf(record.payload["partId"]) ?? "",
    position: { x: positionX, y: positionY },
    rotation: numberOf(record.payload["rotation"]) ?? 0,
    isMirror: record.payload["isMirror"] === true || record.payload["isMirror"] === 1,
  };
  for (const key of COMPONENT_ATTR_KEYS) {
    const value = attrValue(attrs, key);
    if (value === undefined) continue;
    if (key === "Designator") component.ref = value;
    else if (key === "Value") component.value = value;
    else if (key === "Device") component.partNumber = value;
    else if (key === "Footprint") component.footprint = value;
    else component.symbolUuid = value;
  }
  return component;
}

function parseSymbolPart(
  records: readonly EasyedaRecord[],
  attrs: Map<string, EasyedaRecord[]>,
): EasyedaSymbolPart[] {
  const byPart = new Map<string, EasyedaPin[]>();
  for (const record of records) {
    if (record.type !== "PIN" || record.id === undefined) continue;
    const positionX = numberOf(record.payload["positionX"]);
    const positionY = numberOf(record.payload["positionY"]);
    if (positionX === undefined || positionY === undefined) continue;
    const own = attrs.get(record.id) ?? [];
    const number = attrValue(own, "NUMBER");
    if (number === undefined) continue; // PIN 必须有 NUMBER，缺失的分不了引脚号。
    const name = attrValue(own, "NAME");
    const partId = textOf(record.payload["partId"]) ?? "";
    const pin: EasyedaPin = { number, ...(name === undefined ? {} : { name }), x: positionX, y: positionY };
    const pins = byPart.get(partId);
    if (pins) pins.push(pin);
    else byPart.set(partId, [pin]);
  }
  return [...byPart].map(([partId, pins]) => ({ partId, pins, symbolUuid: "" }));
}

function parseWire(records: readonly EasyedaRecord[], attrs: EasyedaRecord[]): EasyedaWire | undefined {
  const record = records[0];
  if (record === undefined || record.id === undefined) return undefined;
  const netName = attrValue(attrs, "NET");
  return {
    id: record.id,
    isBus: record.type === "BUS",
    ...(netName === undefined ? {} : { netName }),
    segments: parsePoints(record.payload["dots"]),
  };
}

function groupedByType(records: readonly EasyedaRecord[]): Map<string, EasyedaRecord[]> {
  const grouped = new Map<string, EasyedaRecord[]>();
  for (const record of records) {
    const list = grouped.get(record.type);
    if (list) list.push(record);
    else grouped.set(record.type, [record]);
  }
  return grouped;
}

function symbolPartsOf(document: EasyedaDocument): EasyedaSymbolPart[] {
  const grouped = groupedByType(document.records);
  const attrs = attrsByParent(document.records);
  return parseSymbolPart(grouped.get("PIN") ?? [], attrs).map((part) => ({
    ...part,
    symbolUuid: document.uuid,
  }));
}

/**
 * 解析一份 `.esch2`。`pageUuid` 指定要读的 `SCH_PAGE` 文档；省略时取文件里第一个
 * （多页文件请调用方按页面 uuid 指定，避免读错页）。
 */
export function parseEasyedaSchematic(text: string, pageUuid?: string): EasyedaSchematic {
  const documents = decodeEasyedaLog(text);
  if (documents.length === 0)
    throw new CircuitServiceError("unsupported_format", "不是 EasyEDA V3 日志：没有 DOCHEAD");

  const symbols = documents
    .filter((document) => document.docType === "SYMBOL")
    .flatMap((document) => symbolPartsOf(document));

  const pages = documents.filter((document) => document.docType === "SCH_PAGE");
  const page =
    pageUuid === undefined ? pages[0] : pages.find((document) => document.uuid === pageUuid);
  if (page === undefined)
    throw new CircuitServiceError(
      pageUuid === undefined ? "unsupported_format" : "not_found",
      pageUuid === undefined
        ? "不是 EasyEDA 原理图页：没有 SCH_PAGE 文档"
        : `找不到原理图页 ${pageUuid}`,
    );

  const grouped = groupedByType(page.records);
  const attrs = attrsByParent(page.records);
  const components = (grouped.get("COMPONENT") ?? [])
    .map((record) => parseComponent([record], attrs.get(record.id ?? "") ?? []))
    .filter((component): component is EasyedaComponent => component !== undefined);
  const wires = [...(grouped.get("WIRE") ?? []), ...(grouped.get("BUS") ?? [])]
    .map((record) => parseWire([record], attrs.get(record.id ?? "") ?? []))
    .filter((wire): wire is EasyedaWire => wire !== undefined);

  return { pageUuid: page.uuid, components, symbols, wires };
}
