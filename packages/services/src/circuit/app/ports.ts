import type { CircuitIR } from "@escode/shared/circuit";
import type { CircuitCorrectionRecord } from "../domain/circuitCorrections.js";
import type { CircuitLayout } from "../contract.js";

/**
 * CircuitService 的端口（app 层只依赖这些接口，具体 IO 在 adapters 里实现）。
 * 目录与文件布局见 §3.2：Host 应用数据目录 `circuits/<hash(identityKey)>/<hash(sourcePath)>/`。
 */

/** 修正记录（corrections.json 的一条，定义在 domain/circuitCorrections.ts）：作者、时间、目标元素、修正内容。 */
export type { CircuitCorrectionRecord, CorrectionTarget } from "../domain/circuitCorrections.js";

export interface StoredCircuit {
  irId: string;
  ir: CircuitIR;
  /** 渲染几何，与 ir.revision 对齐；netlist / BOM 这类没有几何的来源没有该字段。 */
  layout?: CircuitLayout;
  corrections: CircuitCorrectionRecord[];
}

export interface CircuitStorePort {
  read(params: { identityKey: string; sourcePath: string }): Promise<StoredCircuit | undefined>;
  /**
   * 按 irId 反查该 workspace 下的记录：irId 由 (identityKey, sourcePath) 派生，无法反解，
   * 所以服务重启后（Agent 还拿着旧 irId）要能扫目录找回来。找不到返回 undefined。
   */
  findByIrId(params: { identityKey: string; irId: string }): Promise<StoredCircuit | undefined>;
  write(params: {
    identityKey: string;
    sourcePath: string;
    record: StoredCircuit;
  }): Promise<void>;
}

export interface CircuitSourcePort {
  /** 读 workspace 内的文本文件；不存在抛 `not_found`，其余 IO 失败抛 `io_error`。 */
  readText(params: { workspacePath: string; relativePath: string }): Promise<string>;
  exists(params: { workspacePath: string; relativePath: string }): Promise<boolean>;
  /** sha256 十六进制内容哈希（§3.2 用它区分图片/PDF 的版本，也用于判断源文件是否变化）。 */
  hashContent(text: string): string;
}

export interface CircuitSourceWatch {
  dispose(): void;
}

export interface CircuitSourceWatcherPort {
  /**
   * 监听单个源文件的变化（去掉防抖后再回调）。文件被删除、替换也要回调一次，
   * 由服务标记 stale；监听失败（文件不存在、平台不支持）时抛错，由调用方决定是否降级。
   */
  watch(params: {
    workspacePath: string;
    relativePath: string;
    onChange: () => void;
  }): Promise<CircuitSourceWatch>;
}

export interface CircuitServicePorts {
  store: CircuitStorePort;
  source: CircuitSourcePort;
  watcher: CircuitSourceWatcherPort;
}
