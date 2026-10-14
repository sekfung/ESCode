/**
 * 抓包后端解析（docs/specs/usb-agent-tools.md §4）。
 *
 * 优先级：`builtin`（随包分发的自研抓包器）→ `dumpcap`（用户自装 Wireshark 时的兼容后端）→ 无。
 * 这一层只负责「谁能抓、为什么不能抓、怎么修」，不负责抓包本身。
 */
export type CaptureBackendId = "builtin" | "dumpcap";
export interface CaptureInterfaceRef {
    id: string;
    bus?: number;
    label: string;
}
export interface BuiltinProbe {
    available: boolean;
    usbmonModuleLoaded: boolean;
    privilege: "ok" | "denied" | "unknown";
    interfaces: CaptureInterfaceRef[];
    reason?: string;
    remediation?: string;
}
export interface CaptureBackendStatus {
    id: CaptureBackendId;
    available: boolean;
    command?: string;
    privilege: "ok" | "denied" | "unknown";
    interfaces: CaptureInterfaceRef[];
    reason?: string;
    remediation?: string;
    /** 仅 `builtin`：helper 自报的探测结果（Windows 服务缺失时为 undefined）。 */
    helper?: BuiltinProbe;
}
export interface CaptureBackends {
    /** 实际抓包会用的后端；没有可用后端时为 undefined。 */
    preferred?: CaptureBackendId;
    backends: CaptureBackendStatus[];
}
export interface BackendProbeInput {
    env: Record<string, string | undefined>;
    platform?: NodeJS.Platform;
}
/**
 * 解析自研抓包器路径。三级回退，顺序即优先级：
 * 1. env 显式覆盖（开发与测试用）；
 * 2. 插件包内 `bin/<platform>-<arch>/`（官方插件 seed 会带着它，安装包无需额外配置）；
 * 3. Electron 资源目录的 `tools/`（与 cua-helper 同构的投放位置）。
 */
export declare function resolveHelperCandidates(input: {
    env: Record<string, string | undefined>;
    platform?: NodeJS.Platform;
    arch?: string;
    resourcesPath?: string;
}): string[];
/** 取第一个真实存在且可执行的候选；都没有时返回 undefined 与用于报错的候选清单。 */
export declare function resolveHelperPath(input: Parameters<typeof resolveHelperCandidates>[0]): Promise<{
    path?: string;
    tried: string[];
}>;
/** 调用 helper 的 `--probe`，把它的自报结果当成后端状态。 */
export declare function probeBuiltinHelper(helperPath: string, env: Record<string, string | undefined>): Promise<CaptureBackendStatus>;
export declare function resolveCaptureBackends(input: BackendProbeInput): Promise<CaptureBackends>;
/** 抓包时选择后端：优先内置，其次 dumpcap；都不可用时用**首选不可用后端**的错误信息报错。 */
export declare function selectCaptureBackend(backends: CaptureBackends): {
    backend?: CaptureBackendStatus;
    failure?: CaptureBackendStatus;
};
//# sourceMappingURL=backends.d.ts.map