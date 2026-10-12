import type { CircuitComponent, CircuitDiagnostic, CircuitNet } from "@escode/shared/circuit";
import {
  computePageNets,
  findPageNetByName,
  UnionFind,
  type PageTerminal,
} from "./kicadConnectivity.js";
import {
  dedupePinRefs,
  dirName,
  joinWorkspacePath,
  makeNetId,
  synthesizeNetName,
} from "./circuitNaming.js";
import {
  symbolPins,
  type KicadSchematicDocument,
  type KicadSchematicSymbol,
} from "./kicadSchematic.js";

/**
 * 层次化设计的展开与网络合并（规格 §3.4、§4）。
 *
 * - 根图纸实例路径是 `/<rootUuid>`，子图纸追加 `/<\u5b50\u56fe\u7eb8 uuid>`；元素 id 去掉根 uuid 前缀：
 *   根页元素 id 就是 KiCad UUID，子图纸元素 id 形如 `/<sheetUuid>/<elementUuid>`。
 * - 同名 hierarchy label 与父页 sheet pin 相连；global label 与电源符号跨页面按名字合并；
 *   local label 只在所在图纸实例内有效（同一个子图纸被实例化两次时各自成网）。
 * - 电源符号不进 components，只贡献网络名（KiCad 网表同样排除电源符号）。
 * - 总线成员未展开；`.kicad_pcb` 没有导线，网络来自 pad 上的 `(net …)`（见 kicadPcb.ts）。
 */

export interface KicadDesignInput {
  /** 根图纸的 workspace 相对路径。 */
  rootPath: string;
  /** 根图纸 uuid（KiCad 实例路径的第一段）。 */
  rootUuid: string;
  /** 图纸文件路径（workspace 相对、"/" 分隔）→ 文档；调用方负责把用到的文件都读进来。 */
  documents: Map<string, KicadSchematicDocument>;
}

export interface KicadDesign {
  components: CircuitComponent[];
  nets: CircuitNet[];
  diagnostics: CircuitDiagnostic[];
  /** 解析限制（总线、未加载的子图纸、层级成环），供 CONTRACT.md 声明的 P1 边界使用。 */
  limitations: { kind: "bus" | "unresolved-sheet" | "sheet-cycle"; detail: string }[];
}

/** 一张已加载的图纸实例（DFS 先序；渲染几何也用它遍历图纸）。`relativePath` 去掉根 uuid 前缀，根图纸为 `""`。 */
export interface SheetInstance {
  instancePath: string;
  relativePath: string;
  filePath: string;
  document: KicadSchematicDocument;
  sheetUuid: string;
}

interface ComponentGroup {
  key: string;
  instance: SheetInstance;
  componentId: string;
  reference: string;
  sourceUuids: string[];
  value?: string;
  footprint?: string;
  partNumber?: string;
  pins: { number: string; name?: string; point: { x: number; y: number } }[];
  declaredPinCount: number;
  expectedPinCount: number;
}

const PARSER_ORIGIN = { by: "parser", confidence: 1 } as const;

export function buildKicadDesign(input: KicadDesignInput): KicadDesign {
  const limitations: KicadDesign["limitations"] = [];
  const instances = collectSheetInstances(input, limitations);
  const components: CircuitComponent[] = [];
  const groups: ComponentGroup[] = [];
  const componentIdByKey = new Map<string, string>();
  const terminalsByInstance = new Map<string, PageTerminal[]>();

  for (const instance of instances) {
    const terminals: PageTerminal[] = [];
    for (const group of groupSymbolsByComponent(instance, input.rootUuid)) {
      groups.push(group);
      componentIdByKey.set(group.key, group.componentId);
      components.push(buildComponent(group));
      for (const pin of group.pins)
        terminals.push({
          kind: "pin",
          point: pin.point,
          componentKey: group.key,
          pinNumber: pin.number,
        });
    }
    for (const label of instance.document.labels)
      terminals.push({
        kind:
          label.kind === "local"
            ? "label"
            : label.kind === "global"
              ? "global-label"
              : "hierarchical-label",
        name: label.text,
        point: { x: label.at.x, y: label.at.y },
      });
    for (const sheet of instance.document.sheets)
      for (const pin of sheet.pins)
        terminals.push({ kind: "sheet-pin", name: pin.name, point: { x: pin.at.x, y: pin.at.y } });
    for (const power of collectPowerTerminals(instance)) terminals.push(power);
    terminalsByInstance.set(instance.instancePath, terminals);
    if (instance.document.hasBuses)
      limitations.push({ kind: "bus", detail: `${instance.filePath} 含总线，未展开总线成员` });
  }

  const connectivityByInstance = new Map<string, ReturnType<typeof computePageNets>>();
  for (const instance of instances)
    connectivityByInstance.set(
      instance.instancePath,
      computePageNets({
        wires: instance.document.wires,
        junctions: instance.document.junctions,
        terminals: terminalsByInstance.get(instance.instancePath) ?? [],
      }),
    );

  const uf = new UnionFind();
  const nodeFor = (key: string): string => {
    uf.add(key);
    return key;
  };

  for (const instance of instances) {
    const connectivity = connectivityByInstance.get(instance.instancePath)!;
    for (const [index, net] of connectivity.nets.entries()) {
      const node = nodeFor(`${instance.instancePath}#${index}`);
      for (const terminal of net.terminals) {
        if (!terminal.name) continue;
        if (terminal.kind === "global-label" || terminal.kind === "power")
          // global label 与电源符号：跨所有实例按名字合并。
          uf.union(`global:${terminal.kind}:${terminal.name}`, node);
        else if (terminal.kind === "label" || terminal.kind === "hierarchical-label")
          // local / hierarchy label：同名只在同一图纸实例内相通（KiCad 的标签语义）。
          uf.union(`scoped:${instance.instancePath}:${terminal.kind}:${terminal.name}`, node);
      }
    }
  }
  // 子图纸 hierarchy label ↔ 父页 sheet pin（多实例时按各自实例路径分别合并）。
  for (const instance of instances) {
    if (instance.relativePath === "") continue;
    const parentInstance = instances.find(
      (candidate) =>
        candidate.instancePath ===
        instance.instancePath.slice(0, instance.instancePath.lastIndexOf("/")),
    );
    if (!parentInstance) continue;
    const sheet = parentInstance.document.sheets.find(
      (candidate) => candidate.uuid === instance.sheetUuid,
    );
    if (!sheet) continue;
    const parentConnectivity = connectivityByInstance.get(parentInstance.instancePath)!;
    const childConnectivity = connectivityByInstance.get(instance.instancePath)!;
    for (const sheetPin of sheet.pins) {
      const parentNet = findPageNetByName(parentConnectivity, ["sheet-pin"], sheetPin.name);
      const childNet = findPageNetByName(childConnectivity, ["hierarchical-label"], sheetPin.name);
      if (!parentNet || !childNet) continue;
      uf.union(
        nodeFor(`${parentInstance.instancePath}#${parentConnectivity.nets.indexOf(parentNet)}`),
        nodeFor(`${instance.instancePath}#${childConnectivity.nets.indexOf(childNet)}`),
      );
    }
  }

  // 汇总：按并查集根分组 → 定名字 → 定 id → 回填引脚 netId。
  const drafts: {
    pinRefs: { componentId: string; pin: string }[];
    globalNames: string[];
    scopedNames: string[];
    scopes: string[];
  }[] = [];
  const draftIndexByRoot = new Map<string, number>();
  for (const instance of instances) {
    const connectivity = connectivityByInstance.get(instance.instancePath)!;
    for (const [index, net] of connectivity.nets.entries()) {
      const root = uf.find(nodeFor(`${instance.instancePath}#${index}`));
      let draftIndex = draftIndexByRoot.get(root);
      if (draftIndex === undefined) {
        draftIndex = drafts.length;
        drafts.push({ pinRefs: [], globalNames: [], scopedNames: [], scopes: [] });
        draftIndexByRoot.set(root, draftIndex);
      }
      const draft = drafts[draftIndex]!;
      draft.scopes.push(instance.relativePath);
      for (const pin of net.pins) {
        const componentId = componentIdByKey.get(pin.componentKey);
        if (componentId) draft.pinRefs.push({ componentId, pin: pin.pinNumber });
      }
      for (const terminal of net.terminals) {
        if (!terminal.name) continue;
        if (terminal.kind === "global-label" || terminal.kind === "power")
          draft.globalNames.push(terminal.name);
        else if (terminal.kind === "label" || terminal.kind === "hierarchical-label")
          draft.scopedNames.push(terminal.name);
      }
    }
  }
  const componentById = new Map(components.map((component) => [component.id, component]));
  const nets: CircuitNet[] = drafts
    .map((draft) => ({
      pinRefs: dedupePinRefs(draft.pinRefs),
      globalNames: [...new Set(draft.globalNames)].sort(),
      scopedNames: [...new Set(draft.scopedNames)].sort(),
      scopes: [...new Set(draft.scopes)].sort(),
    }))
    .filter((draft) => draft.pinRefs.length > 0 || draft.globalNames.length > 0 || draft.scopedNames.length > 0)
    .map((draft) => {
      const scoped = draft.globalNames.length === 0;
      const name =
        draft.globalNames[0] ??
        draft.scopedNames[0] ??
        synthesizeNetName(
          draft.pinRefs.map((pinRef) => ({
            ref: componentById.get(pinRef.componentId)?.ref ?? "?",
            pinNumber: pinRef.pin,
          })),
        );
      return {
        id: makeNetId(name, draft.pinRefs, scoped ? draft.scopes[0] : undefined),
        name,
        pinRefs: draft.pinRefs,
        origin: PARSER_ORIGIN,
      };
    })
    .sort((a, b) => a.id.localeCompare(b.id));

  const netIdByPin = new Map<string, string>();
  for (const net of nets)
    for (const pinRef of net.pinRefs)
      netIdByPin.set(`${pinRef.componentId}\u0000${pinRef.pin}`, net.id);
  for (const component of components)
    for (const pin of component.pins) {
      const netId = netIdByPin.get(`${component.id}\u0000${pin.number}`);
      if (netId) pin.netId = netId;
    }

  const diagnostics: CircuitDiagnostic[] = [];
  for (const group of groups) {
    // 只在源文件真的声明了引脚时比较（KiCad 6 的实例不含 (pin …) 声明）。
    if (group.declaredPinCount > 0 && group.declaredPinCount !== group.expectedPinCount)
      diagnostics.push({
        code: "pin-count-mismatch",
        severity: "warning",
        message: `器件 ${group.reference} 声明 ${group.declaredPinCount} 个引脚，库符号按单元给出 ${group.expectedPinCount} 个`,
        componentIds: [group.componentId],
      });
  }
  return { components, nets, diagnostics, limitations };
}

/**
 * 层次展开：从根图纸出发按 DFS 先序收集所有**已加载**的图纸实例。
 * 子图纸文件缺失记 `unresolved-sheet`，层级重复出现记 `sheet-cycle`（两者都只记录、不递归）；
 * 渲染（kicadLayout）与网络推导（buildKicadDesign）都必须走同一条展开路径，保证元素 id 前缀一致。
 */
export function collectSheetInstances(
  input: KicadDesignInput,
  limitations: KicadDesign["limitations"],
): SheetInstance[] {
  const rootDocument = input.documents.get(input.rootPath);
  if (!rootDocument) throw new Error(`not_found: 根图纸 ${input.rootPath} 未加载`);
  const instances: SheetInstance[] = [];
  const visit = (
    filePath: string,
    document: KicadSchematicDocument,
    instancePath: string,
    relativePath: string,
    ancestors: string[],
  ) => {
    instances.push({
      instancePath,
      relativePath,
      filePath,
      document,
      sheetUuid: instancePath.slice(instancePath.lastIndexOf("/") + 1),
    });
    for (const sheet of document.sheets) {
      const childPath = joinWorkspacePath(dirName(filePath), sheet.fileName);
      const childDocument = input.documents.get(childPath);
      if (!childDocument) {
        limitations.push({ kind: "unresolved-sheet", detail: `子图纸 ${sheet.fileName} 未加载` });
        continue;
      }
      if (ancestors.includes(childPath)) {
        limitations.push({ kind: "sheet-cycle", detail: `${childPath} 在层级链上重复出现` });
        continue;
      }
      visit(
        childPath,
        childDocument,
        `${instancePath}/${sheet.uuid}`,
        `${relativePath}/${sheet.uuid}`,
        [...ancestors, childPath],
      );
    }
  };
  visit(input.rootPath, rootDocument, `/${input.rootUuid}`, "", [input.rootPath]);
  return instances;
}

function groupSymbolsByComponent(instance: SheetInstance, rootUuid: string): ComponentGroup[] {
  const groups = new Map<string, ComponentGroup>();
  for (const symbol of instance.document.symbols) {
    const libSymbol = instance.document.libSymbols.get(symbol.libId);
    if (libSymbol?.power) continue;
    const reference = resolveReference(symbol, instance, rootUuid);
    const key = `${instance.instancePath}\u0000${reference}`;
    const pins = symbolPins(symbol, libSymbol).map((pin) => ({
      number: pin.number,
      name: pin.name,
      point: pin.point,
    }));
    const existing = groups.get(key);
    if (existing) {
      existing.pins.push(...pins);
      existing.sourceUuids.push(symbol.uuid);
      existing.declaredPinCount += symbol.declaredPins.length;
      existing.expectedPinCount += pins.length;
      continue;
    }
    const prefix = instance.relativePath === "" ? "" : `${instance.relativePath}/`;
    groups.set(key, {
      key,
      instance,
      componentId: `${prefix}${symbol.uuid}`,
      reference,
      sourceUuids: [symbol.uuid],
      value: symbol.properties.get("Value") || undefined,
      footprint: symbol.properties.get("Footprint") || undefined,
      partNumber: symbol.properties.get("MPN") || undefined,
      pins,
      declaredPinCount: symbol.declaredPins.length,
      expectedPinCount: pins.length,
    });
  }
  return [...groups.values()];
}

function buildComponent(group: ComponentGroup): CircuitComponent {
  return {
    id: group.componentId,
    ref: group.reference,
    ...(group.value ? { value: group.value } : {}),
    ...(group.partNumber ? { partNumber: group.partNumber } : {}),
    ...(group.footprint ? { footprint: group.footprint } : {}),
    pins: group.pins.map((pin) => ({
      number: pin.number,
      ...(pin.name && pin.name !== "~" ? { name: pin.name } : {}),
    })),
    origin: PARSER_ORIGIN,
  };
}

/** 位号解析：优先当前实例路径（KiCad 7/8 的 `(instances …)`），再退回 KiCad 6 的 `(symbol_instances …)` 与 Reference 属性。 */
export function resolveReference(
  symbol: KicadSchematicSymbol,
  instance: { instancePath: string; document: KicadSchematicDocument },
  rootUuid: string,
): string {
  const withoutRoot = instance.instancePath.slice(`/${rootUuid}`.length);
  for (const path of [instance.instancePath, withoutRoot]) {
    if (!path) continue;
    const modern = symbol.instanceReferences.get(path);
    if (modern) return modern;
    const legacy = instance.document.legacySymbolInstances.get(path);
    if (legacy) return legacy;
  }
  return symbol.properties.get("Reference") ?? "?";
}

function collectPowerTerminals(instance: SheetInstance): PageTerminal[] {
  const terminals: PageTerminal[] = [];
  for (const symbol of instance.document.symbols) {
    const libSymbol = instance.document.libSymbols.get(symbol.libId);
    if (!libSymbol?.power) continue;
    const name = symbol.properties.get("Value");
    if (!name) continue;
    for (const pin of symbolPins(symbol, libSymbol))
      terminals.push({ kind: "power", name, point: pin.point });
  }
  return terminals;
}
