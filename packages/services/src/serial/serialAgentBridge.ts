import {
  buildSerialSendPayload,
  createSerialStreamDecoder,
  formatSerialHex,
  type SerialDisplayEncoding,
  type SerialToolOp,
  type ZCodeSerialCancelParams,
  type ZCodeSerialMethodParams,
  type ZCodeSerialMethodResult,
} from "@zcode/shared/serial";
import { SerialError, type SerialChunk, type SerialConfig, type SerialStatus } from "./serial.js";
import type { SerialService } from "./serialService.js";

/** wait_for 超时/断开时附带的最近 RX 文本长度。 */
const WAIT_TAIL_CHARS = 2048;
/** wait_for 匹配成功时返回的前文长度。 */
const WAIT_CONTEXT_CHARS = 512;
/** tail 计算时读取的 RX 原始字节上限（覆盖 Host 环形缓冲）。 */
const TAIL_SOURCE_BYTES = 1024 * 1024;

export interface SerialAgentBridge {
  handle<Op extends SerialToolOp>(
    op: Op,
    params: ZCodeSerialMethodParams<Op>,
  ): Promise<ZCodeSerialMethodResult<Op>>;
  /** 终止同一会话中仍在进行的 waitFor；目标不存在或已结束时为空操作（幂等）。 */
  cancel(params: ZCodeSerialCancelParams): void;
}

interface DecodedSegment {
  seq: number;
  end: number;
}

function decodeChunks(chunks: readonly SerialChunk[], encoding: SerialDisplayEncoding) {
  const decoder = createSerialStreamDecoder(encoding);
  const segments: DecodedSegment[] = [];
  let text = "";
  for (const chunk of chunks) {
    text += decoder.decode(chunk.bytes);
    segments.push({ seq: chunk.seq, end: text.length });
  }
  return { text, segments };
}

function formatRead(
  chunks: readonly SerialChunk[],
  direction: "rx" | "tx" | "both",
  encoding: "utf-8" | "gbk" | "hex",
): string {
  if (direction !== "both") {
    return encoding === "hex"
      ? chunks.map((chunk) => formatSerialHex(chunk.bytes)).join(" ")
      : decodeChunks(chunks, encoding).text;
  }
  // 双向时与导出一致：每段一行并带方向前缀；收发各用独立流式解码器，跨段多字节字符不被拆坏。
  const decoders =
    encoding === "hex"
      ? null
      : { rx: createSerialStreamDecoder(encoding), tx: createSerialStreamDecoder(encoding) };
  return chunks
    .map((chunk) => {
      const body = decoders
        ? decoders[chunk.direction].decode(chunk.bytes)
        : formatSerialHex(chunk.bytes);
      const line = `${chunk.direction.toUpperCase()} ${body}`;
      return line.endsWith("\n") ? line : `${line}\n`;
    })
    .join("");
}

function sameConfig(
  status: SerialStatus,
  path: string,
  args: ZCodeSerialMethodParams<"open">["args"],
) {
  const config = status.config;
  return (
    status.path === path &&
    config !== undefined &&
    config.baudRate === args.baudRate &&
    config.dataBits === args.dataBits &&
    config.parity === args.parity &&
    config.stopBits === args.stopBits &&
    config.rtscts === args.rtscts
  );
}

function describeCurrent(status: SerialStatus): string {
  const config = status.config;
  const params = config
    ? `${config.baudRate} ${config.dataBits}${config.parity[0]?.toUpperCase()}${config.stopBits}${config.rtscts ? " RTS/CTS" : ""}`
    : "";
  return `${status.path ?? "unknown"} (${status.state}${params ? `, ${params}` : ""})`;
}

/**
 * Host 侧 Agent 串口工具处理：参数已由协议 schema 严格校验。
 * 串口状态的唯一所有者仍是 SerialService，这里只做权限边界（本地会话、不抢占）与编解码。
 */
export function createSerialAgentBridge(options: {
  getSerialService: () => SerialService | undefined;
  /** 用户为该串口记住的自动重连偏好；Agent 打开串口时继承，缺省为 true。 */
  getRememberedAutoReconnect: (path: string) => Promise<boolean | undefined>;
}): SerialAgentBridge {
  const waits = new Map<string, { sessionId: string; controller: AbortController }>();

  function requireService(params: { remoteSessionId?: string }): SerialService {
    const service = options.getSerialService();
    // 串口设备属于本机窗口：远程 workspace 的会话即使误把请求送到这里也不能操作本机串口。
    if (!service || params.remoteSessionId) {
      throw new SerialError("unavailable", "Serial port tools are not available here");
    }
    return service;
  }

  async function currentStatus(service: SerialService): Promise<SerialStatus> {
    return (await service.getSnapshot()).status;
  }

  async function recentRxTail(service: SerialService, encoding: SerialDisplayEncoding) {
    const { chunks } = service.readSince({
      sinceSeq: 0,
      direction: "rx",
      maxBytes: TAIL_SOURCE_BYTES,
    });
    return decodeChunks(chunks, encoding).text.slice(-WAIT_TAIL_CHARS);
  }

  const handlers: {
    [Op in SerialToolOp]: (
      service: SerialService,
      params: ZCodeSerialMethodParams<Op>,
    ) => Promise<ZCodeSerialMethodResult<Op>>;
  } = {
    async list(service) {
      const ports = await service.list();
      return { ports, status: await currentStatus(service) };
    },

    async open(service, { args }) {
      const status = await currentStatus(service);
      if (status.state !== "closed" && status.state !== "error") {
        // 不抢占：只复用与当前完全一致的 path+参数，避免 Agent 切走用户正在使用的串口。
        if (status.state === "open" && sameConfig(status, args.path, args)) {
          return { reused: true, status };
        }
        throw new SerialError(
          "busy",
          `Serial port is already in use: ${describeCurrent(status)}. Ask the user to close it first.`,
        );
      }
      const config: SerialConfig = {
        baudRate: args.baudRate,
        dataBits: args.dataBits,
        parity: args.parity,
        stopBits: args.stopBits,
        rtscts: args.rtscts,
        autoReconnect: (await options.getRememberedAutoReconnect(args.path)) ?? true,
      };
      await service.open({ path: args.path, config });
      return { reused: false, status: await currentStatus(service) };
    },

    async write(service, { args, sessionId }) {
      const payload = buildSerialSendPayload({
        input: args.data,
        mode: args.encoding === "hex" ? "hex" : "text",
        lineEnding: args.lineEnding,
      });
      if (!payload.ok) {
        throw new SerialError("invalidInput", `Invalid HEX data: ${payload.error}`);
      }
      const { seq } = await service.write({ bytes: payload.bytes, source: "agent", sessionId });
      return { bytes: payload.bytes.byteLength, seq };
    },

    async read(service, { args }) {
      const result = service.readSince(args);
      return {
        text: formatRead(result.chunks, args.direction, args.encoding),
        lastSeq: result.lastSeq,
        truncated: result.truncated,
        evicted: result.evicted,
        status: await currentStatus(service),
      };
    },

    async waitFor(service, { args, requestId, sessionId }) {
      let pattern: RegExp;
      try {
        pattern = new RegExp(args.pattern, args.flags);
      } catch (error) {
        throw new SerialError(
          "invalidInput",
          `Invalid pattern: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
      const controller = new AbortController();
      waits.set(requestId, { sessionId, controller });
      try {
        const outcome = await service.waitFor({
          ...(args.sinceSeq !== undefined ? { sinceSeq: args.sinceSeq } : {}),
          timeoutMs: args.timeoutMs,
          signal: controller.signal,
          test: (chunks) => {
            const { text, segments } = decodeChunks(chunks, args.encoding);
            const match = pattern.exec(text);
            if (!match) return null;
            const end = match.index + match[0].length;
            const seq = segments.find((segment) => segment.end >= end)?.seq ?? 0;
            return {
              match: match[0],
              context: text.slice(Math.max(0, match.index - WAIT_CONTEXT_CHARS), match.index),
              seq,
            };
          },
        });
        if (outcome.kind === "matched") return { matched: true, ...outcome.value };
        return {
          matched: false,
          reason:
            outcome.kind === "aborted"
              ? "cancelled"
              : outcome.kind === "timeout"
                ? "timeout"
                : "disconnected",
          tail: await recentRxTail(service, args.encoding),
          lastSeq: outcome.lastSeq,
        };
      } finally {
        waits.delete(requestId);
      }
    },

    async close(service) {
      if ((await currentStatus(service)).state !== "closed") await service.close();
      return { status: await currentStatus(service) };
    },
  };

  return {
    async handle(op, params) {
      const service = requireService(params);
      const handler = handlers[op] as (
        service: SerialService,
        params: ZCodeSerialMethodParams<typeof op>,
      ) => Promise<ZCodeSerialMethodResult<typeof op>>;
      return handler(service, params);
    },
    cancel({ sessionId, targetRequestId }) {
      const wait = waits.get(targetRequestId);
      // 只允许发起等待的会话取消，避免一个会话终止另一个会话的等待。
      if (wait?.sessionId === sessionId) wait.controller.abort();
    },
  };
}
