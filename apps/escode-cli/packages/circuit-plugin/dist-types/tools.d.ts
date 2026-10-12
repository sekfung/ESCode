import { type CircuitAgentToolOp } from "@escode/shared/circuit";
/** MCP 工具元数据：readOnlyHint 让只读工具在 plan 模式可用；写类工具标 destructive，plan 模式不放行。 */
export interface CircuitToolDefinition {
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
export declare const CIRCUIT_TOOLS: CircuitToolDefinition[];
export type CircuitBrokerSend = (request: {
    op: CircuitAgentToolOp;
    args: unknown;
    sessionId: string;
    turnId?: string;
    runtimeScope: "main" | "subagent";
}, signal: AbortSignal) => Promise<{
    ok: true;
    result: unknown;
} | {
    ok: false;
    error: {
        code: string;
        message: string;
    };
}>;
export interface CircuitToolResult {
    content: Array<{
        type: "text";
        text: string;
    }>;
    isError?: true;
}
export declare function handleCircuitToolCall(input: {
    name: string;
    args: unknown;
    meta: Record<string, unknown> | undefined;
    send: CircuitBrokerSend;
    signal: AbortSignal;
}): Promise<CircuitToolResult>;
//# sourceMappingURL=tools.d.ts.map