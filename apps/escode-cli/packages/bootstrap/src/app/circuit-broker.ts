import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { rm } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Logger, McpServerConfig } from "@escode/contracts";
import {
  CIRCUIT_BROKER_SOCKET_ENV,
  CIRCUIT_BROKER_TOKEN_ENV,
  CIRCUIT_MCP_SERVER_NAME,
  circuitBrokerRequestSchema,
  type CircuitAgentToolOp,
  type CircuitBrokerRequest,
  type CircuitBrokerResponse,
  type CircuitToolRequest,
} from "@escode/shared/circuit";

const MAX_REQUEST_BYTES = 1024 * 1024;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;

/** Agent 运行时到 Host 的电路通道；实现见 escode-protocol/circuit-control-broker.ts。 */
export interface CircuitControlPort {
  request<Op extends CircuitAgentToolOp>(
    op: Op,
    args: CircuitToolRequest<Op>,
    context: { sessionId: string; turnId?: string; signal?: AbortSignal },
  ): Promise<unknown>;
}

export interface CircuitBroker {
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
 * circuit MCP server 与 Agent 运行时之间的私有 broker（docs/circuit/README.md §5）。
 * 与 serial broker 同构：一行 JSON 请求、常量时间 token 校验、拒绝子 agent；
 * 差别是电路工具都是短请求（没有 wait_for 那类长等待），因此不需要取消方法。
 * 会话归属由 CircuitControlPort 的 requireSession 做权威校验。
 */
export function createCircuitBroker(input: {
  port: CircuitControlPort;
  logger: Logger;
  platform?: NodeJS.Platform | string;
}): CircuitBroker {
  const platform = input.platform ?? process.platform;
  const socketPath =
    platform === "win32"
      ? `\\\\.\\pipe\\escode-circuit-${randomUUID()}`
      : join(tmpdir(), `zsc-${randomUUID()}.sock`);
  const token = randomBytes(32).toString("hex");
  const server = createServer((socket) => {
    handleSocket(socket, { ...input, token }).catch((error) => {
      input.logger.warn("Circuit broker request failed", {
        event: "circuit.broker.request.failed",
        error: error instanceof Error ? error.message : String(error),
      });
    });
  });
  server.on("error", (error) => {
    input.logger.error("Circuit broker failed", error, { event: "circuit.broker.failed" });
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

export function injectCircuitBroker(
  servers: Record<string, McpServerConfig>,
  broker: Pick<CircuitBroker, "socketPath" | "token"> | undefined,
): Record<string, McpServerConfig> {
  const circuit = servers[CIRCUIT_MCP_SERVER_NAME];
  if (!broker || !circuit || circuit.type !== "stdio") return servers;
  // 连接材料只定向注入 circuit server，绝不进入 Agent 全局 env（避免其它 MCP/Bash 冒用 IR 访问权）。
  return {
    ...servers,
    [CIRCUIT_MCP_SERVER_NAME]: {
      ...circuit,
      env: {
        ...circuit.env,
        [CIRCUIT_BROKER_SOCKET_ENV]: broker.socketPath,
        [CIRCUIT_BROKER_TOKEN_ENV]: broker.token,
      },
    },
  };
}

async function handleSocket(
  socket: Socket,
  input: { port: CircuitControlPort; logger: Logger; token: string },
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
    const parsed = circuitBrokerRequestSchema.safeParse(payload);
    // 请求不合工具契约是 broker 级协议失败，不是 §5 的 IR 错误码：照 serial 用 invalidInput，
    // MCP server 侧按 `[invalidInput] …` 文本报告（规格错误码只描述服务端工具结果）。
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

function authorize(request: CircuitBrokerRequest, token: string): void {
  const actual = Buffer.from(request.token);
  const expected = Buffer.from(token);
  if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
    throw new BrokerRequestError("unavailable", "Circuit broker request is not authorized");
  }
  // 子 agent 不操作共享 IR：避免多个子 agent 并发改写同一份电路状态。
  if (request.runtimeScope === "subagent") {
    throw new BrokerRequestError("unavailable", "Circuit tools are not available in subagents");
  }
}

function toBrokerError(error: unknown): { code: string; message: string } {
  if (error instanceof BrokerRequestError) return { code: error.code, message: error.message };
  const message = error instanceof Error ? error.message : String(error);
  // Host 侧协议错误经 JSON-RPC error.data.code 带回（见 services circuitAgentRequestRouter）。
  const data = (error as { data?: unknown } | null)?.data;
  const code =
    data && typeof data === "object" && typeof (data as { code?: unknown }).code === "string"
      ? (data as { code: string }).code
      : "io_error";
  return { code, message };
}

function requestIdFromPayload(payload: unknown): string | undefined {
  const id = (payload as { id?: unknown } | null)?.id;
  return typeof id === "string" && UUID_PATTERN.test(id) ? id : undefined;
}

function respond(socket: Socket, response: CircuitBrokerResponse): void {
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
        reject(new BrokerRequestError("too_large", "Circuit broker request exceeded 1 MiB"));
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
