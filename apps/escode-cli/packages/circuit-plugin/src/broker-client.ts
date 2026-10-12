import { randomUUID } from "node:crypto";
import { createConnection } from "node:net";
import {
  CIRCUIT_BROKER_SOCKET_ENV,
  CIRCUIT_BROKER_TOKEN_ENV,
  circuitBrokerResponseSchema,
} from "@escode/shared/circuit";
import type { CircuitBrokerSend } from "./tools.js";

/** 单次响应上限：分页结果本身就是 64 KiB 级，JSON 转义后仍远小于此。 */
const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;

export interface CircuitBrokerConnection {
  socketPath: string;
  token: string;
}

/**
 * 在 main() 生命周期内捕获：宿主只把 broker 连接材料定向注入本 server 的 env。
 * 缺失表示本进程不是由 ESCode 宿主按电路能力启动的（纯 CLI/TUI），所有调用返回 unavailable。
 */
export function captureCircuitBrokerConnection(
  env: NodeJS.ProcessEnv = process.env,
): CircuitBrokerConnection | undefined {
  const socketPath = env[CIRCUIT_BROKER_SOCKET_ENV]?.trim();
  const token = env[CIRCUIT_BROKER_TOKEN_ENV]?.trim();
  return socketPath && token ? { socketPath, token } : undefined;
}

export function createCircuitBrokerSend(
  connection: CircuitBrokerConnection | undefined,
): CircuitBrokerSend {
  return async (request, signal) => {
    if (!connection) {
      return {
        ok: false,
        error: {
          code: "unavailable",
          // §3.5：纯 CLI/TUI 没有 Host 与 broker，不提供降级引擎，只提示用户去 Desktop。
          message:
            "Circuit tools need the ESCode Desktop app with this workspace open (请在 ESCode Desktop 中使用)",
        },
      };
    }
    const id = randomUUID();
    return await new Promise((resolve, reject) => {
      let buffer = "";
      let settled = false;
      const socket = createConnection(connection.socketPath);
      const finish = (error: unknown, value?: Awaited<ReturnType<CircuitBrokerSend>>) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        // 断开连接即通知 broker 取消在途请求。
        socket.destroy();
        if (value) resolve(value);
        else reject(error);
      };
      const onAbort = () => finish(new DOMException("aborted", "AbortError"));
      signal.addEventListener("abort", onAbort, { once: true });
      socket.once("connect", () => {
        socket.write(`${JSON.stringify({ id, token: connection.token, ...request })}\n`);
      });
      socket.on("data", (chunk) => {
        buffer += chunk.toString("utf8");
        if (Buffer.byteLength(buffer) > MAX_RESPONSE_BYTES) {
          finish(new Error("Circuit broker response exceeded the size limit"));
          return;
        }
        const newline = buffer.indexOf("\n");
        if (newline < 0) return;
        try {
          const parsed = circuitBrokerResponseSchema.parse(JSON.parse(buffer.slice(0, newline)));
          if (parsed.id !== id) throw new Error("Circuit broker response id mismatch");
          finish(
            undefined,
            parsed.ok ? { ok: true, result: parsed.result } : { ok: false, error: parsed.error },
          );
        } catch (error) {
          finish(error);
        }
      });
      socket.once("error", (error) => finish(error));
      socket.once("close", () => finish(new Error("Circuit broker closed the connection")));
    });
  };
}
