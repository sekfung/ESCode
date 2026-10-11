import type { CircuitBrokerSend } from "./tools.js";
export interface CircuitBrokerConnection {
    socketPath: string;
    token: string;
}
/**
 * 在 main() 生命周期内捕获：宿主只把 broker 连接材料定向注入本 server 的 env。
 * 缺失表示本进程不是由 ESCode 宿主按电路能力启动的（纯 CLI/TUI），所有调用返回 unavailable。
 */
export declare function captureCircuitBrokerConnection(env?: NodeJS.ProcessEnv): CircuitBrokerConnection | undefined;
export declare function createCircuitBrokerSend(connection: CircuitBrokerConnection | undefined): CircuitBrokerSend;
//# sourceMappingURL=broker-client.d.ts.map