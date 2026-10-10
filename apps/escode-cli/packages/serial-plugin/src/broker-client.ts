import { randomUUID } from "node:crypto";
import { createConnection } from "node:net";
import {
  SERIAL_BROKER_SOCKET_ENV,
  SERIAL_BROKER_TOKEN_ENV,
  serialBrokerResponseSchema,
} from "@escode/shared/serial";
import type { SerialBrokerSend } from "./tools.js";

/** read 结果上限 32 KiB 原始字节，转义/JSON 后仍远小于此。 */
const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;

export interface SerialBrokerConnection {
  socketPath: string;
  token: string;
}

/**
 * 在 main() 生命周期内捕获：宿主只把 broker 连接材料定向注入本 server 的 env。
 * 缺失表示本进程不是由 ESCode 宿主按串口能力启动的，所有调用返回 unavailable。
 */
export function captureSerialBrokerConnection(
  env: NodeJS.ProcessEnv = process.env,
): SerialBrokerConnection | undefined {
  const socketPath = env[SERIAL_BROKER_SOCKET_ENV]?.trim();
  const token = env[SERIAL_BROKER_TOKEN_ENV]?.trim();
  return socketPath && token ? { socketPath, token } : undefined;
}

export function createSerialBrokerSend(
  connection: SerialBrokerConnection | undefined,
): SerialBrokerSend {
  return async (request, signal) => {
    if (!connection) {
      return {
        ok: false,
        error: { code: "unavailable", message: "Serial port tools are not available here" },
      };
    }
    const id = randomUUID();
    return await new Promise((resolve, reject) => {
      let buffer = "";
      let settled = false;
      const socket = createConnection(connection.socketPath);
      const finish = (error: unknown, value?: Awaited<ReturnType<SerialBrokerSend>>) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        // 断开连接即通知 broker 取消在途请求（wait_for 会让 Host 释放订阅）。
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
          finish(new Error("Serial broker response exceeded the size limit"));
          return;
        }
        const newline = buffer.indexOf("\n");
        if (newline < 0) return;
        try {
          const parsed = serialBrokerResponseSchema.parse(JSON.parse(buffer.slice(0, newline)));
          if (parsed.id !== id) throw new Error("Serial broker response id mismatch");
          finish(
            undefined,
            parsed.ok ? { ok: true, result: parsed.result } : { ok: false, error: parsed.error },
          );
        } catch (error) {
          finish(error);
        }
      });
      socket.once("error", (error) => finish(error));
      socket.once("close", () => finish(new Error("Serial broker closed the connection")));
    });
  };
}
