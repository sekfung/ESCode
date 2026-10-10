import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { rm } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Logger, McpServerConfig } from "@zcode/contracts";
import {
  SERIAL_BROKER_SOCKET_ENV,
  SERIAL_BROKER_TOKEN_ENV,
  SERIAL_MCP_SERVER_NAME,
  serialBrokerRequestSchema,
  type SerialBrokerRequest,
  type SerialBrokerResponse,
  type SerialToolArgs,
  type SerialToolOp,
} from "@zcode/shared/serial";

const MAX_REQUEST_BYTES = 1024 * 1024;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;

/** Agent 运行时到 Host 的串口通道；实现见 zcode-protocol/serial-control-broker.ts。 */
export interface SerialControlPort {
  request<Op extends SerialToolOp>(
    op: Op,
    args: SerialToolArgs<Op>,
    context: { sessionId: string; turnId?: string; signal?: AbortSignal },
  ): Promise<unknown>;
}

export interface SerialBroker {
  close(): Promise<void>;
  ready: Promise<void>;
  socketPath: string;
  token: string;
}

class BrokerRequestError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/**
 * serial MCP server 与 Agent 运行时之间的私有 broker（docs/specs/serial-agent-tools.md）。
 * 与 node_repl browser broker 同构：一行 JSON 请求、常量时间 token 校验、拒绝子 agent；
 * 会话归属由 SerialControlPort 的 requireSession 做权威校验。
 */
export function createSerialBroker(input: {
  port: SerialControlPort;
  logger: Logger;
  platform?: NodeJS.Platform | string;
}): SerialBroker {
  const platform = input.platform ?? process.platform;
  const socketPath =
    platform === "win32"
      ? `\\\\.\\pipe\\zcode-serial-${randomUUID()}`
      : join(tmpdir(), `zsr-${randomUUID()}.sock`);
  const token = randomBytes(32).toString("hex");
  const server = createServer((socket) => {
    handleSocket(socket, { ...input, token }).catch((error) => {
      input.logger.warn("Serial broker request failed", {
        event: "serial.broker.request.failed",
        error: error instanceof Error ? error.message : String(error),
      });
    });
  });
  server.on("error", (error) => {
    input.logger.error("Serial broker failed", error, { event: "serial.broker.failed" });
  });
  server.listen(socketPath);
  server.unref();
  const ready = new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  return {
    ready,
    socketPath,
    token,
    close: async () => {
      await ready.catch(() => undefined);
      await closeServer(server);
      if (platform !== "win32") await rm(socketPath, { force: true });
    },
  };
}

export function injectSerialBroker(
  servers: Record<string, McpServerConfig>,
  broker: Pick<SerialBroker, "socketPath" | "token"> | undefined,
): Record<string, McpServerConfig> {
  const serial = servers[SERIAL_MCP_SERVER_NAME];
  if (!broker || !serial || serial.type !== "stdio") return servers;
  // 连接材料只定向注入 serial server，绝不进入 Agent 全局 env（避免其它 MCP/Bash 冒用串口）。
  return {
    ...servers,
    [SERIAL_MCP_SERVER_NAME]: {
      ...serial,
      env: {
        ...serial.env,
        [SERIAL_BROKER_SOCKET_ENV]: broker.socketPath,
        [SERIAL_BROKER_TOKEN_ENV]: broker.token,
      },
    },
  };
}

async function handleSocket(
  socket: Socket,
  input: { port: SerialControlPort; logger: Logger; token: string },
): Promise<void> {
  const abortController = new AbortController();
  let completed = false;
  let requestId: string = randomUUID();
  // 对端（MCP 调用被取消）断开时取消在途请求；监听覆盖整个生命周期，避免 EPIPE 冒泡成进程级错误。
  socket.on("error", () => {
    if (!completed) abortController.abort();
  });
  socket.once("close", () => {
    if (!completed) abortController.abort();
  });
  try {
    const payload = JSON.parse(await readLine(socket, abortController.signal)) as unknown;
    requestId = requestIdFromPayload(payload) ?? requestId;
    const parsed = serialBrokerRequestSchema.safeParse(payload);
    if (!parsed.success) {
      throw new BrokerRequestError("invalidInput", parsed.error.message);
    }
    authorize(parsed.data, input.token);
    const result = await input.port.request(parsed.data.op, parsed.data.args as never, {
      sessionId: parsed.data.sessionId,
      ...(parsed.data.turnId ? { turnId: parsed.data.turnId } : {}),
      signal: abortController.signal,
    });
    completed = true;
    respond(socket, { id: requestId, ok: true, result });
  } catch (error) {
    completed = true;
    respond(socket, { id: requestId, ok: false, error: toBrokerError(error) });
  }
}

function authorize(request: SerialBrokerRequest, token: string): void {
  const actual = Buffer.from(request.token);
  const expected = Buffer.from(token);
  if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
    throw new BrokerRequestError("unauthorized", "Serial broker request is not authorized");
  }
  // 子 agent 不得操作硬件：避免多个子 agent 同时往同一串口写入。
  if (request.runtimeScope === "subagent") {
    throw new BrokerRequestError("unavailable", "Serial port tools are not available in subagents");
  }
}

function toBrokerError(error: unknown): { code: string; message: string } {
  if (error instanceof BrokerRequestError) return { code: error.code, message: error.message };
  const message = error instanceof Error ? error.message : String(error);
  // Host 的串口业务失败经 JSON-RPC error.data.code 带回（见 services serialAgentRequestRouter）。
  const data = (error as { data?: unknown } | null)?.data;
  const code =
    data && typeof data === "object" && typeof (data as { code?: unknown }).code === "string"
      ? (data as { code: string }).code
      : "io";
  return { code, message };
}

function requestIdFromPayload(payload: unknown): string | undefined {
  const id = (payload as { id?: unknown } | null)?.id;
  return typeof id === "string" && UUID_PATTERN.test(id) ? id : undefined;
}

function respond(socket: Socket, response: SerialBrokerResponse): void {
  if (!socket.writable) return;
  socket.end(`${JSON.stringify(response)}\n`);
}

async function readLine(socket: Socket, signal: AbortSignal): Promise<string> {
  return await new Promise<string>((resolve, reject) => {
    let buffer = "";
    const cleanup = () => {
      socket.off("data", onData);
      socket.off("error", onError);
      signal.removeEventListener("abort", onAbort);
    };
    const onData = (chunk: Buffer) => {
      buffer += chunk.toString("utf8");
      if (Buffer.byteLength(buffer) > MAX_REQUEST_BYTES) {
        cleanup();
        reject(new BrokerRequestError("invalidInput", "Serial broker request exceeded 1 MiB"));
        return;
      }
      const newline = buffer.indexOf("\n");
      if (newline < 0) return;
      cleanup();
      resolve(buffer.slice(0, newline));
    };
    const onError = (error: Error) => {
      cleanup();
      reject(error);
    };
    const onAbort = () => {
      cleanup();
      reject(new DOMException("aborted", "AbortError"));
    };
    socket.on("data", onData);
    socket.once("error", onError);
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

async function closeServer(server: Server): Promise<void> {
  if (!server.listening) return;
  await new Promise<void>((resolve, reject) => {
    server.close((error) => (error ? reject(error) : resolve()));
  });
}
