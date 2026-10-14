/**
 * USB 工具定义与调用处理（docs/specs/usb-agent-tools.md §3）。
 *
 * 三个工具构成单向漏斗「探 → 抓 → 解」，职责不重叠；description 里写清**什么时候该用它**，
 * 因为模型选错工具多半是描述里只写了能力、没写触发条件。
 */
export interface UsbToolDefinition {
    name: string;
    description: string;
    inputSchema: {
        type: "object";
        [key: string]: unknown;
    };
    annotations: {
        readOnlyHint?: boolean;
        destructiveHint?: boolean;
        title: string;
    };
}
export interface UsbToolResult {
    content: Array<{
        type: "text";
        text: string;
    }>;
    isError?: true;
}
export interface UsbToolContext {
    env: Record<string, string | undefined>;
    workspace: string;
    pluginData: string;
    signal: AbortSignal;
}
export declare const USB_TOOL_NAMES: readonly ["usb_probe", "usb_capture", "usb_decode"];
export type UsbToolName = (typeof USB_TOOL_NAMES)[number];
/** 免审批清单，需与官方插件定义的 `defaultAllowedTools` 一致（有测试钉住）。 */
export declare const USB_READ_ONLY_TOOLS: readonly UsbToolName[];
export declare const USB_TOOLS: UsbToolDefinition[];
/** 工具失败：code 与 serial 工具同一套命名，便于模型跨插件复用经验。 */
export declare class ToolFailure extends Error {
    readonly code: string;
    readonly remediation?: string | undefined;
    constructor(code: string, message: string, remediation?: string | undefined);
}
export declare function handleUsbToolCall(input: {
    name: string;
    args: unknown;
    context: UsbToolContext;
}): Promise<UsbToolResult>;
//# sourceMappingURL=tools.d.ts.map