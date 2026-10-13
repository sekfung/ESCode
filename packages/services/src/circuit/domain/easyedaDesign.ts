import type { CircuitComponent, CircuitNet, CircuitPin } from "@escode/shared/circuit";
import { makeNetId } from "./circuitNaming.js";
import type { ParsedCircuitElements } from "./irBuild.js";
import type { EasyedaComponent, EasyedaPin, EasyedaSchematic } from "./easyedaSchematic.js";

/**
 * EasyEDA 原理图 → IR 元素（§4）：器件来自 COMPONENT + 属性，网络由「导线几何连通 + NET 名合并」推出。
 *
 * 连通性口径（文档 `format/schematic/component.md` 的变换顺序）：符号坐标里的引脚连接点先绕原点
 * 逆时针旋转 `rotation`，再按 `isMirror` 绕 Y 轴镜像，最后平移 `position` 得到页坐标；页坐标与导线
 * `dots` 同一坐标系。引脚落在导线上即属同一网络；同名 `NET` 的多段导线按名字合并（与 KiCad 的标签同义）。
 *
 * 已声明边界：不做符号库解析（符号必须在同一份 `.esch2` 里）；没有 Designator 的器件（电源/网络标志等）
 * 跳过；`BUS` 按普通导线参与连通，不做总线成员展开；不产出渲染几何。
 */

const PARSER_ORIGIN = { by: "parser", confidence: 1 } as const;

/** 页坐标取整精度：坐标单位是 0.01 inch，引脚与导线都在整数格点上，浮点旋转后按 1 个单位吸附。 */
const POINT_TOLERANCE = 1;

function pointKey(x: number, y: number): string {
  return `${Math.round(x / POINT_TOLERANCE)}\u0000${Math.round(y / POINT_TOLERANCE)}`;
}

function transformPoint(point: { x: number; y: number }, component: EasyedaComponent): { x: number; y: number } {
  const radians = (component.rotation * Math.PI) / 180;
  const cos = Math.cos(radians);
  const sin = Math.sin(radians);
  const rotatedX = point.x * cos - point.y * sin;
  const rotatedY = point.x * sin + point.y * cos;
  const mirroredX = component.isMirror ? -rotatedX : rotatedX;
  return { x: mirroredX + component.position.x, y: rotatedY + component.position.y };
}

/** 并查集：键是吸附后的坐标点，导线把相邻点连起来。 */
class PointSets {
  private readonly parent = new Map<string, string>();

  find(key: string): string {
    const existing = this.parent.get(key);
    if (existing === undefined) {
      this.parent.set(key, key);
      return key;
    }
    if (existing === key) return key;
    const root = this.find(existing);
    this.parent.set(key, root);
    return root;
  }

  union(left: string, right: string): void {
    const leftRoot = this.find(left);
    const rightRoot = this.find(right);
    if (leftRoot !== rightRoot) this.parent.set(rightRoot, leftRoot);
  }
}

interface ComponentPins {
  component: EasyedaComponent;
  pins: EasyedaPin[];
}

function resolvePins(
  schematic: EasyedaSchematic,
  components: EasyedaComponent[],
): ComponentPins[] {
  const bySymbol = new Map<string, Map<string, EasyedaPin[]>>();
  for (const part of schematic.symbols) {
    const parts = bySymbol.get(part.symbolUuid) ?? new Map<string, EasyedaPin[]>();
    parts.set(part.partId, part.pins);
    bySymbol.set(part.symbolUuid, parts);
  }
  return components.map((component) => ({
    component,
    pins:
      component.symbolUuid === undefined
        ? []
        : (bySymbol.get(component.symbolUuid)?.get(component.partId) ?? []),
  }));
}

/**
 * 器件（COMPONENT + 属性）与网络。网络先按导线几何连通分组，再按 `NET` 名把同名的组并起来。
 */
export function elementsFromEasyedaSchematic(schematic: EasyedaSchematic): ParsedCircuitElements {
  const sets = new PointSets();
  const namesByPoint = new Map<string, string>();
  const rootByName = new Map<string, string>();

  for (const wire of schematic.wires) {
    for (const segment of wire.segments) {
      for (let index = 0; index + 1 < segment.length; index += 1) {
        const from = segment[index]!;
        const to = segment[index + 1]!;
        sets.union(pointKey(from.x, from.y), pointKey(to.x, to.y));
      }
      if (wire.netName !== undefined)
        for (const point of segment) namesByPoint.set(pointKey(point.x, point.y), wire.netName);
    }
    if (wire.netName === undefined) continue;
    // 同名 NET 的多段导线是同一个网（标签语义）：把各自的根并到一起。
    const first = wire.segments[0]?.[0];
    if (first === undefined) continue;
    const root = sets.find(pointKey(first.x, first.y));
    const existing = rootByName.get(wire.netName);
    if (existing === undefined) rootByName.set(wire.netName, root);
    else sets.union(existing, root);
  }

  // 所有合并做完之后再解析「根 → 网络名」，避免记下中途会被重新挂载的根。
  const nameByRoot = new Map<string, string>();
  for (const [key, name] of namesByPoint) nameByRoot.set(sets.find(key), name);

  const withRef = schematic.components.filter(
    (component): component is EasyedaComponent & { ref: string } => component.ref !== undefined,
  );
  const pinsByComponent = resolvePins(schematic, withRef);

  /** 网络根 → 引脚引用；同时记下每个根上出现的网络名。 */
  const pinRefsByRoot = new Map<string, { componentId: string; pin: string }[]>();
  const components: CircuitComponent[] = [];
  for (const entry of pinsByComponent) {
    const component: CircuitComponent = {
      id: `easyeda:${entry.component.id}`,
      ref: entry.component.ref!,
      ...(entry.component.value === undefined ? {} : { value: entry.component.value }),
      ...(entry.component.partNumber === undefined ? {} : { partNumber: entry.component.partNumber }),
      ...(entry.component.footprint === undefined ? {} : { footprint: entry.component.footprint }),
      pins: entry.pins.map((pin) => ({ number: pin.number, ...(pin.name === undefined ? {} : { name: pin.name }) })),
      origin: PARSER_ORIGIN,
    };
    for (const pin of entry.pins) {
      const point = transformPoint(pin, entry.component);
      const root = sets.find(pointKey(point.x, point.y));
      const refs = pinRefsByRoot.get(root);
      const ref = { componentId: component.id, pin: pin.number };
      if (refs) refs.push(ref);
      else pinRefsByRoot.set(root, [ref]);
    }
    components.push(component);
  }
  components.sort((left, right) => left.ref.localeCompare(right.ref));

  const nets: CircuitNet[] = [];
  for (const [root, pinRefs] of pinRefsByRoot) {
    if (pinRefs.length === 0) continue; // 只有引脚、没有导线时也不构成网络。
    const name = nameByRoot.get(root);
    const sorted = [...pinRefs].sort(
      (left, right) =>
        left.componentId.localeCompare(right.componentId) || left.pin.localeCompare(right.pin),
    );
    nets.push({
      id: makeNetId(name, sorted),
      ...(name === undefined ? {} : { name }),
      pinRefs: sorted,
      origin: PARSER_ORIGIN,
    });
  }
  nets.sort((left, right) => left.id.localeCompare(right.id));

  // 引脚回填 netId：IR 里引脚与网络必须互相引用（§4）。
  const netIdByPin = new Map<string, string>();
  for (const net of nets)
    for (const pinRef of net.pinRefs)
      netIdByPin.set(`${pinRef.componentId}\u0000${pinRef.pin}`, net.id);
  for (const component of components)
    component.pins = component.pins.map((pin): CircuitPin => {
      const netId = netIdByPin.get(`${component.id}\u0000${pin.number}`);
      return netId === undefined ? pin : { ...pin, netId };
    });

  return { components, nets, blocks: [], diagnostics: [] };
}
