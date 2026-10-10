import type { SerialBrokerSend } from "./tools.js";
export interface SerialBrokerConnection {
    socketPath: string;
    token: string;
}
/**
 * 在 main() 生命周期内捕获：宿主只把 broker 连接材料定向注入本 server 的 env。
 * 缺失表示本进程不是由 ZCode 宿主按串口能力启动的，所有调用返回 unavailable。
 */
export declare function captureSerialBrokerConnection(env?: NodeJS.ProcessEnv): SerialBrokerConnection | undefined;
export declare function createSerialBrokerSend(connection: SerialBrokerConnection | undefined): SerialBrokerSend;
//# sourceMappingURL=broker-client.d.ts.map