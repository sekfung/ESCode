/**
 * 实时抓包（docs/specs/usb-agent-tools.md §3.3 / §5）。
 *
 * 只有一个后端：Windows 上的 `escode-usb-capture` 服务。插件是**作业所有者**——它负责参数校验、
 * 超时、取消与把结果翻译成工具结果；抓包器只接受固定参数、写一个 pcapng 文件、回报一行 JSON。
 * Linux 与 macOS 没有实时抓包，只有离线解码（`backends.ts` 的注释里写了为什么不做 dumpcap 回退）。
 */
import { type ProbeCapture } from "./probe.js";
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
    backend: "builtin";
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
    env: Record<string, string | undefined>;
}): Promise<CaptureOutcome>;
//# sourceMappingURL=capture.d.ts.map