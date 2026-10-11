import type {
  CircuitLayout,
  CircuitLayoutPin,
  CircuitLayoutShape,
  CircuitLayoutSheet,
  CircuitLayoutSymbol,
  CircuitLayoutText,
} from "../contract.js";
import {
  collectSheetInstances,
  type KicadDesignInput,
  type SheetInstance,
} from "./kicadDesign.js";
import {
  transformSymbolPoint,
  type KicadLibPin,
  type KicadLibSymbol,
  type KicadSchematicSymbol,
  type KicadSymbolProperty,
} from "./kicadSchematic.js";
import type { KicadPcbDocument, KicadPcbFootprint } from "./kicadPcb.js";

/**
 * 渲染几何（规格 §8「KiCad 根据解析出的几何信息自己渲染 SVG」）：把已解析的文档模型拼成
 * `CircuitLayout` 契约里的只读派生视图。纯 domain、零 IO；不带任何 SVG/视图细节，面板自己画。
 *
 * 坐标与单位（全部 mm）：
 * - 库坐标（lib_symbols 里的图形与引脚，+Y 向上）→ 图纸坐标（+Y 向下）统一走
 *   `transformSymbolPoint`，顺序是「先按 (at … rotation) 旋转、再按 (mirror …) 镜像、最后平移」
 *   （组合次序由真实板子的落点实测决定，见该函数注释）。
 * - 引脚体的另一端必须先在库坐标里算出来再整体变换，不能在图纸坐标里直接加向量，否则镜像/旋转下方向会错。
 * - 导线、结点、标签、实例 property 的坐标本来就写在图纸坐标里，原样搬运。
 * - 元素 id 前缀与 `kicadDesign` 的组件 id 规则一致：根图纸 `uuid`，子图纸 `/<sheetUuid>/uuid`。
 */

/** 原理图渲染几何的输入：几何只依赖 KicadDesignInput，IR 的 irId/revision 由 CircuitService 对齐（缺省占位）。 */
export interface KicadSchematicLayoutInput extends KicadDesignInput {
  irId?: string;
  revision?: number;
}

export function buildKicadSchematicLayout(input: KicadSchematicLayoutInput): CircuitLayout {
  // 限制项（未加载子图纸、层级成环）由 IR 侧的 buildKicadDesign 记录，这里只渲染已加载的图纸；
  // 展开路径复用同一个 collectSheetInstances，保证 layout 与 IR 的元素 id 前缀逐字一致。
  const instances = collectSheetInstances(input, []);
  const sheetNameByInstance = new Map<string, string>();
  for (const instance of instances)
    for (const sheet of instance.document.sheets)
      if (sheet.name) sheetNameByInstance.set(`${instance.instancePath}/${sheet.uuid}`, sheet.name);
  return {
    irId: input.irId ?? "",
    revision: input.revision ?? 0,
    sheets: instances.map((instance) =>
      buildSchematicSheet(instance, sheetNameByInstance.get(instance.instancePath)),
    ),
  };
}

function buildSchematicSheet(
  instance: SheetInstance,
  name: string | undefined,
): CircuitLayoutSheet {
  const document = instance.document;
  const prefix = instance.relativePath === "" ? "" : `${instance.relativePath}/`;
  return {
    path: instance.relativePath,
    // 子图纸取父页 sheet 的 Sheetname；根图纸没有父页，省略。
    ...(name === undefined ? {} : { name }),
    page: { width: document.page.width, height: document.page.height },
    symbols: document.symbols.map((symbol) =>
      buildSchematicSymbol(symbol, document.libSymbols.get(symbol.libId), prefix),
    ),
    wires: document.wires.map((wire) => ({ points: wire.points.map(toPoint) })),
    junctions: document.junctions.map(toPoint),
    labels: document.labels.map((label) => ({
      text: label.text,
      x: label.at.x,
      y: label.at.y,
      rotation: label.at.rotation,
      kind: label.kind,
    })),
  };
}

/** 复制成普通点：同时隔离 layout 与解析文档，避免面板拿到共享的可变对象。 */
function toPoint(point: { x: number; y: number }): { x: number; y: number } {
  return { x: point.x, y: point.y };
}

function buildSchematicSymbol(
  symbol: KicadSchematicSymbol,
  libSymbol: KicadLibSymbol | undefined,
  prefix: string,
): CircuitLayoutSymbol {
  const power = libSymbol?.power === true;
  const powerName = power ? symbol.properties.get("Value") : undefined;
  return {
    // 电源符号不进 IR（没有 component），只留网络名；其余符号的 elementId 与 IR 组件 id 逐字一致。
    ...(power ? {} : { elementId: `${prefix}${symbol.uuid}` }),
    ...(powerName === undefined ? {} : { powerName }),
    libId: symbol.libId,
    unit: symbol.unit,
    x: symbol.at.x,
    y: symbol.at.y,
    rotation: symbol.at.rotation,
    ...(symbol.mirror ? { mirror: symbol.mirror } : {}),
    shapes: libSymbol ? buildShapes(symbol, libSymbol) : [],
    pins: libSymbol ? buildPins(symbol, libSymbol) : [],
    texts: symbol.propertyPlacements.map(toLayoutText),
  };
}

/** 实例单元的库图形（unit 0 = 所有单元共用）：每个点按实例的 mirror/rotation 变换到图纸坐标。 */
function buildShapes(
  symbol: KicadSchematicSymbol,
  libSymbol: KicadLibSymbol,
): CircuitLayoutShape[] {
  return libSymbol.graphics
    .filter((graphic) => graphic.unit === 0 || graphic.unit === symbol.unit)
    .map((graphic) => ({
      kind: graphic.kind,
      points: graphic.points.map((point) => transformSymbolPoint(symbol, point)),
      filled: graphic.filled,
    }));
}

/**
 * 引脚：`connection` 必须与网络推导用的点完全一致，所以过滤规则（unit 0 共用）和变换函数都与
 * `kicadSchematic.symbolPins` 相同；`body` 是引脚画线的另一端（符号体内侧）。
 */
function buildPins(symbol: KicadSchematicSymbol, libSymbol: KicadLibSymbol): CircuitLayoutPin[] {
  return libSymbol.pins
    .filter((pin) => pin.unit === 0 || pin.unit === symbol.unit)
    .map((pin) => ({
      number: pin.number,
      ...(pin.name === "~" ? {} : { name: pin.name }),
      connection: transformSymbolPoint(symbol, pin.at),
      body: transformSymbolPoint(symbol, pinBodyPoint(pin)),
    }));
}

/**
 * 引脚内侧端点（库坐标）：`(pin … (at x y rot) (length l))` 的 at 是连接点，引脚从连接点沿 rot 方向
 * 画进符号体（0° = +X、90° = +Y；库坐标 +Y 向上）。先在库坐标算好再交给 transformSymbolPoint，
 * 镜像与正交旋转就会同时作用在两端，方向不会算反。
 */
function pinBodyPoint(pin: KicadLibPin): { x: number; y: number } {
  const radians = (pin.at.rotation * Math.PI) / 180;
  return {
    x: pin.at.x + pin.length * Math.cos(radians),
    y: pin.at.y + pin.length * Math.sin(radians),
  };
}

/** 实例 property → 文本元素：property 的 `(at …)` 就是图纸坐标；Reference/Value 之外归为 property。 */
function toLayoutText(property: KicadSymbolProperty): CircuitLayoutText {
  return {
    kind:
      property.name === "Reference"
        ? "reference"
        : property.name === "Value"
          ? "value"
          : "property",
    text: property.value,
    x: property.at.x,
    y: property.at.y,
    rotation: property.at.rotation,
    ...(property.hidden ? { hidden: true } : {}),
  };
}

export interface KicadPcbLayoutInput {
  pcb: KicadPcbDocument;
  irId: string;
  revision: number;
  /** 已构建 IR 的组件（id ↔ ref）：把 footprint 映射回 IR 元素，PCB 里 ref 是唯一定位键。 */
  components: { id: string; ref: string }[];
}

/**
 * `.kicad_pcb` 渲染几何：单张图纸（path 为空串，没有 Sheetname 级名称），page 取板框包围盒。
 * PCB 没有原理图那样的符号图形，只能画 footprint 的焊盘位置与位号文本。
 */
export function buildKicadPcbLayout(input: KicadPcbLayoutInput): CircuitLayout {
  const elementIdByRef = new Map<string, string>();
  for (const component of input.components)
    if (!elementIdByRef.has(component.ref)) elementIdByRef.set(component.ref, component.id);
  return {
    irId: input.irId,
    revision: input.revision,
    sheets: [
      {
        path: "",
        page: input.pcb.outline ?? { width: 0, height: 0 },
        symbols: input.pcb.footprints.map((footprint) =>
          buildPcbSymbol(footprint, elementIdByRef.get(footprint.ref)),
        ),
        wires: [],
        junctions: [],
        labels: [],
      },
    ],
  };
}

/**
 * footprint → 渲染符号：libId 取封装名 `(footprint "Lib:Name")`，位号文本取 Reference。
 * 当前 KicadPcbFootprint 不保留 `(pad … (at dx dy …))` 偏移，pad 自身坐标记 0，
 * 连接点与引脚体都落在 footprint 原点上（面板据此画焊盘标记）。
 */
function buildPcbSymbol(
  footprint: KicadPcbFootprint,
  elementId: string | undefined,
): CircuitLayoutSymbol {
  const { x, y, rotation } = footprint.at;
  return {
    ...(elementId === undefined ? {} : { elementId }),
    libId: footprint.footprint ?? "",
    unit: 1,
    x,
    y,
    rotation,
    shapes: [],
    pins: footprint.pads.map((pad) => ({
      number: pad.number,
      connection: { x, y },
      body: { x, y },
    })),
    texts: [{ kind: "reference", text: footprint.ref, x, y, rotation }],
  };
}
