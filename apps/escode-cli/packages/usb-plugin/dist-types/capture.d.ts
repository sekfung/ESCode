/**
 * 实时抓包（docs/specs/usb-agent-tools.md §3.3 / §5）。
 *
 * 唯一所有者是这里的抓包作业表：一次调用一个子进程，结束时一定收尾（正常退出、工具取消、进程被杀）。
 * 抓包期间写 `.partial`，正常收尾后改名——半截文件被当成完整抓包分析，比失败更糟。
 */
import type { ProbeCapture } from "./probe.js";
export type CaptureErrorCode = "unavailable" | "permission" | "busy" | "invalidInput" | "io" | "invalidConfig";
export declare class CaptureError extends Error {
    readonly code: CaptureErrorCode;
    readonly remediation?: string | undefined;
    constructor(code: CaptureErrorCode, message: string, remediation?: string | undefined);
}
export declare const CAPTURE_MAX_DURATION_MS = 60000;
export declare const CAPTURE_MAX_BYTES: number;
export interface CaptureRequest {
    bus?: number;
    busDev?: string;
    durationMs: number;
    maxBytes: number;
    label?: string;
    outputPath?: string;
}
export interface CaptureOutcome {
    path: string;
    bytes: number;
    packets: number;
    stoppedBy: "duration" | "maxBytes" | "signal";
    backend: string;
    target: {
        bus?: number;
        busDev?: string;
    };
    notes: string[];
}
export declare function runCapture(input: {
    request: CaptureRequest;
    probe: ProbeCapture;
    workspace: string;
    pluginData: string;
    signal: AbortSignal;
}): Promise<CaptureOutcome>;
//# sourceMappingURL=capture.d.ts.map