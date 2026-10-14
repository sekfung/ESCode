/**
 * 环境探测（docs/specs/usb-agent-tools.md §3.2）。
 *
 * 只做**只读**探测：本机能不能抓、用什么后端、为什么不能抓（给可执行的 remediation）、
 * 接了哪些设备、已经有哪些抓包文件。任何提权动作都不在这里做——把命令告诉用户，让他自己决定。
 */
import { type CaptureBackendId, type CaptureBackendStatus, type CaptureInterfaceRef } from "./backends.js";
import { type ProtocolName } from "./types.js";
export interface ProbeDevice {
    busDev?: string;
    bus?: number;
    device?: number;
    vid?: string;
    pid?: string;
    manufacturer?: string;
    product?: string;
    serialNumber?: string;
    interfaces: Array<{
        classCode: number;
        className: string;
        protocols: ProtocolName[];
    }>;
    /** 平台无法给出总线地址时的说明（例如 Windows）。 */
    addressNote?: string;
}
/** 顶层字段取自**首选**后端，便于调用方直接展示；完整候选在 `backends` 里。 */
export interface ProbeCapture {
    available: boolean;
    backend: CaptureBackendId | "none";
    preferred?: CaptureBackendId;
    command?: string;
    interfaces: CaptureInterfaceRef[];
    privilege: "ok" | "denied" | "unknown";
    reason?: string;
    remediation?: string;
    backends: CaptureBackendStatus[];
}
export interface ProbeResult {
    platform: NodeJS.Platform;
    capture: ProbeCapture;
    devices: ProbeDevice[];
    captures: Array<{
        path: string;
        bytes: number;
        modifiedAt: string;
    }>;
    notes: string[];
}
export interface ProbeInput {
    env: Record<string, string | undefined>;
    workspace: string;
    pluginData: string;
}
/** 抓包文件清单：插件数据目录 + 工作区 `.escode/usb-captures`，最近修改的在前。 */
export declare function listCaptureFiles(input: {
    workspace: string;
    pluginData: string;
    limit?: number;
}): Promise<Array<{
    path: string;
    bytes: number;
    modifiedAt: string;
}>>;
export declare function probeEnvironment(input: ProbeInput): Promise<ProbeResult>;
//# sourceMappingURL=probe.d.ts.map