import type { CircuitFirmwareDifference, CircuitIR } from "@escode/shared/circuit";
import { CircuitServiceError } from "./circuitErrors.js";
import { isGroundNetName, isPowerNetName, isSynthesizedNetName } from "./circuitRuleFacts.js";
import {
  canonicalMcuPinName,
  compareFirmwareText,
  dedupeFirmwareDeclarations,
  normalizeFirmwareSignal,
  type FirmwarePinDeclaration,
} from "./firmwarePins.js";

/**
 * 原理图 IR 与固件引脚声明的比对（规格 §7、§7.1）。
 *
 * 比对锚点是 MCU 引脚名：IR 里引脚名能被识别成 `PA9` / `P0.26` / `GPIO12` 的引脚，
 * 其所在网络名与固件声明比对。四类差异：
 * - `pin-mismatch`：固件声明了某引脚的信号，原理图上该引脚接的是**另一个**具名网络；
 * - `firmware-only`：固件声明的引脚在原理图上不存在，或存在但没有网络可对应；
 * - `schematic-only`：原理图 MCU 引脚接了具名网络（不是电源/地、不是 `Net-(…)` 未命名网络），
 *   而固件没有对该引脚的任何声明；条目的 `file` 是原理图源文件（IR 不记录行号）；
 * - `peripheral-conflict`：同一信号落在两个引脚，或同一引脚被两个信号占用；
 *   条目按排序在后的一条声明给出，`file` / `line` 指该条声明。
 *
 * 纯函数、零 IO；结果按 (file, line, kind, pin) 排序，稳定可回归。
 */
export function compareFirmwarePins(
  ir: CircuitIR,
  declarations: FirmwarePinDeclaration[],
): CircuitFirmwareDifference[] {
  const mcuPins = collectMcuPins(ir);
  if (mcuPins.size === 0)
    throw new CircuitServiceError(
      "invalid_ir",
      "IR 中没有可识别的 MCU 引脚名（如 PA9 / P0.26 / GPIO12），无法与固件比对",
    );
  const unique = dedupeFirmwareDeclarations(declarations);
  // 引脚解析：优先用 IR 里真实存在的规范引脚名（数字端口控制器的两种写法都能命中），
  // 命中不了时退回声明里的主写法（`GPIO13`），报 firmware-only。
  const resolved = unique.map((declaration) => ({
    declaration,
    pin: resolveDeclarationPin(declaration, mcuPins),
  }));
  const items: CircuitFirmwareDifference[] = [
    ...conflictItems(resolved),
    ...declarationItems(resolved, mcuPins),
    ...schematicOnlyItems(ir, resolved, mcuPins),
  ];
  return items.sort(
    (a, b) =>
      compareFirmwareText(a.file, b.file) ||
      (a.line ?? 0) - (b.line ?? 0) ||
      compareFirmwareText(a.kind, b.kind) ||
      compareFirmwareText(a.pin ?? "", b.pin ?? "") ||
      compareFirmwareText(a.firmwareSymbol ?? "", b.firmwareSymbol ?? ""),
  );
}

/** 一条声明 + 它在 IR 侧解析出的引脚名（命中不了时为声明的主写法）。 */
interface ResolvedDeclaration {
  declaration: FirmwarePinDeclaration;
  pin: string | undefined;
}

/**
 * 声明引脚 → IR 侧引脚名：优先取「IR 里存在、且网络名与声明信号一致」的候选
 * （数字端口控制器两种写法并存时，用它消歧：Nordic 的 `&gpio1 13` 命中 `P1.13`、
 * ESP 的 `&gpio0 13` 命中 `GPIO13`）；其次取第一个 IR 里存在的候选；
 * 都不存在时返回主写法，用于报 firmware-only。
 */
function resolveDeclarationPin(
  declaration: FirmwarePinDeclaration,
  mcuPins: Map<string, McuPinEntry[]>,
): string | undefined {
  const candidates = [declaration.pin, ...(declaration.pinAliases ?? [])].filter(
    (candidate): candidate is string => candidate !== undefined,
  );
  const unambiguous = candidates.find((candidate) =>
    (mcuPins.get(candidate) ?? []).some((entry) => signalsMatch(entry.netName, declaration.signal)),
  );
  if (unambiguous !== undefined) return unambiguous;
  return candidates.find((candidate) => (mcuPins.get(candidate)?.length ?? 0) > 0) ?? candidates[0];
}

interface McuPinEntry {
  pin: string;
  componentId: string;
  netId?: string;
  netName?: string;
}

/** IR → 规范 MCU 引脚名 → 该引脚的候选（同一引脚名可能出现在多单元器件的多个单元上）。 */
function collectMcuPins(ir: CircuitIR): Map<string, McuPinEntry[]> {
  const netNameById = new Map(ir.nets.map((net) => [net.id, net.name]));
  const pins = new Map<string, McuPinEntry[]>();
  // 器件顺序用码点序：多单元器件共用引脚名时，代表元件的选择要跨平台一致（同 compareFirmwareText）。
  for (const component of [...ir.components].sort((a, b) => compareFirmwareText(a.id, b.id))) {
    for (const pin of component.pins) {
      const name = canonicalMcuPinName(pin.name);
      if (name === undefined) continue;
      const netName = pin.netId === undefined ? undefined : netNameById.get(pin.netId);
      pins.set(name, [
        ...(pins.get(name) ?? []),
        {
          pin: name,
          componentId: component.id,
          ...(pin.netId !== undefined ? { netId: pin.netId } : {}),
          ...(netName !== undefined ? { netName } : {}),
        },
      ]);
    }
  }
  return pins;
}

/** 逐条固件声明对照 IR 的 MCU 引脚：命中即无差异，否则报 mismatch 或 firmware-only。 */
function declarationItems(
  resolved: ResolvedDeclaration[],
  mcuPins: Map<string, McuPinEntry[]>,
): CircuitFirmwareDifference[] {
  const items: CircuitFirmwareDifference[] = [];
  for (const { declaration, pin } of resolved) {
    if (pin === undefined) continue;
    const entries = (mcuPins.get(pin) ?? []).filter((entry) => entry.netName !== undefined);
    if (entries.length === 0) {
      items.push(baseDifference("firmware-only", declaration, pin));
      continue;
    }
    if (entries.some((entry) => signalsMatch(entry.netName, declaration.signal))) continue;
    const representative = entries[0]!;
    items.push({
      ...baseDifference("pin-mismatch", declaration, pin),
      ...(representative.netId !== undefined ? { netId: representative.netId } : {}),
      netName: representative.netName!,
      componentId: representative.componentId,
    });
  }
  return items;
}

/** 原理图独有：IR 上有具名网络的 MCU 引脚没有被任何固件声明覆盖。 */
function schematicOnlyItems(
  ir: CircuitIR,
  resolved: ResolvedDeclaration[],
  mcuPins: Map<string, McuPinEntry[]>,
): CircuitFirmwareDifference[] {
  const declaredPins = new Set(
    resolved.map((item) => item.pin).filter((pin): pin is string => pin !== undefined),
  );
  const items: CircuitFirmwareDifference[] = [];
  for (const [pin, entries] of mcuPins) {
    if (declaredPins.has(pin)) continue;
    const representative = entries.find(
      (entry) => entry.netName !== undefined && !isSchematicOnlyExcluded(entry.netName),
    );
    if (representative?.netName === undefined) continue;
    items.push({
      kind: "schematic-only",
      pin,
      ...(representative.netId !== undefined ? { netId: representative.netId } : {}),
      netName: representative.netName,
      componentId: representative.componentId,
      file: ir.source.path,
    });
  }
  return items;
}

/** 电源/地与未命名网络不是固件外设信号，不参与「原理图独有」判定。 */
function isSchematicOnlyExcluded(netName: string | undefined): boolean {
  return isSynthesizedNetName(netName) || isPowerNetName(netName) || isGroundNetName(netName);
}

/**
 * 固件内部冲突：同一信号落在两个引脚，或同一引脚被两个信号占用。
 * 每个冲突按排序在后的那条声明各报一条；两组建索引可能指向同一条声明，这里按键去重。
 */
function conflictItems(resolved: ResolvedDeclaration[]): CircuitFirmwareDifference[] {
  const byPin = new Map<string, ResolvedDeclaration[]>();
  const bySignal = new Map<string, ResolvedDeclaration[]>();
  for (const item of resolved) {
    if (item.pin === undefined) continue;
    byPin.set(item.pin, [...(byPin.get(item.pin) ?? []), item]);
    bySignal.set(item.declaration.signal, [
      ...(bySignal.get(item.declaration.signal) ?? []),
      item,
    ]);
  }
  const conflicts = new Map<string, ResolvedDeclaration>();
  const collect = (group: ResolvedDeclaration[], distinct: (item: ResolvedDeclaration) => string) => {
    if (new Set(group.map(distinct)).size < 2) return;
    for (const item of [...group].sort(compareDeclarations).slice(1))
      conflicts.set(
        `${item.pin}\u0000${item.declaration.signal}\u0000${item.declaration.file}\u0000${item.declaration.line}`,
        item,
      );
  };
  for (const group of byPin.values()) collect(group, (item) => item.declaration.signal);
  for (const group of bySignal.values()) collect(group, (item) => item.pin ?? "");
  return [...conflicts.values()].map((item) =>
    baseDifference("peripheral-conflict", item.declaration, item.pin),
  );
}

function compareDeclarations(a: ResolvedDeclaration, b: ResolvedDeclaration): number {
  return (
    compareFirmwareText(a.declaration.file, b.declaration.file) ||
    a.declaration.line - b.declaration.line ||
    compareFirmwareText(a.pin ?? "", b.pin ?? "") ||
    compareFirmwareText(a.declaration.signal, b.declaration.signal)
  );
}

function baseDifference(
  kind: CircuitFirmwareDifference["kind"],
  declaration: FirmwarePinDeclaration,
  pin: string | undefined,
): CircuitFirmwareDifference {
  return {
    kind,
    ...(pin !== undefined ? { pin } : {}),
    firmwareSymbol: declaration.signal,
    file: declaration.file,
    line: declaration.line,
  };
}

function signalsMatch(netName: string | undefined, signal: string): boolean {
  return normalizeFirmwareSignal(netName ?? "") === signal;
}
