import { type SerialBrokerRequest } from "@zcode/shared/serial";
/** MCP 工具元数据：readOnlyHint 让只读工具在 plan 模式可用；写类工具标 destructive，plan 模式不放行。 */
export interface SerialToolDefinition {
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
export declare const SERIAL_TOOLS: SerialToolDefinition[];
export type SerialBrokerSend = (request: Omit<SerialBrokerRequest, "id" | "token">, signal: AbortSignal) => Promise<{
    ok: true;
    result: unknown;
} | {
    ok: false;
    error: {
        code: string;
        message: string;
    };
}>;
export interface SerialToolResult {
    content: Array<{
        type: "text";
        text: string;
    }>;
    isError?: true;
}
export declare function handleSerialToolCall(input: {
    name: string;
    args: unknown;
    meta: Record<string, unknown> | undefined;
    send: SerialBrokerSend;
    signal: AbortSignal;
}): Promise<SerialToolResult>;
//# sourceMappingURL=tools.d.ts.map