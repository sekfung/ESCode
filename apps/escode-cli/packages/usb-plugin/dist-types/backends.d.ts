/**
 * 抓包后端（docs/specs/usb-agent-tools.md §4）。
 *
 * 只有**一个**后端：Windows 上随安装包提供的 `escode-usb-capture` 服务（安装时一并装上 USBPcap 驱动）。
 * Linux 与 macOS 不做实时抓包——它们的路径是**离线解码**：用户把在 Windows 机器或硬件分析仪上抓到的
 * `.pcapng` 交给 `usb_decode`。
 *
 * 这里刻意**不探测任何用户自装的第三方抓包工具**（曾经保留的 dumpcap 回退已按产品决定删除）：
 * 「装个 Wireshark 就能抓」会变成一条隐性依赖，而我们要的是「装完 ESCode 就能抓，装不了就明确告诉用户
 * 去别的机器抓文件」。Linux 上如果确实存在自建的抓包器（apps/usb-capture-helper），也会被认出来并使用，
 * 但那不是产品路径、也不给任何安装承诺。
 */
export type CaptureBackendId = "builtin";
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
 * 解析抓包器路径。三级回退，顺序即优先级：
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
/** 调用抓包器的 `--probe`，把它的自报结果当成后端状态。 */
export declare function probeBuiltinHelper(helperPath: string, env: Record<string, string | undefined>): Promise<CaptureBackendStatus>;
export declare function resolveCaptureBackends(input: BackendProbeInput): Promise<CaptureBackends>;
/** 抓包时选择后端；都不可用时把**首选候选**的错误信息交出去（它比调用方更清楚缺什么）。 */
export declare function selectCaptureBackend(backends: CaptureBackends): {
    backend?: CaptureBackendStatus;
    failure?: CaptureBackendStatus;
};
//# sourceMappingURL=backends.d.ts.map