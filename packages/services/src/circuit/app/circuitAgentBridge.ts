import {
  type CircuitAgentToolOp,
  type CircuitToolResponse,
  type ESCodeCircuitMethodParams,
} from "@escode/shared/circuit";
import type { CircuitIrView, ICircuitService } from "../contract.js";
import { CircuitServiceError } from "../domain/circuitErrors.js";

/**
 * Host 侧 Agent 电路工具处理（docs/circuit/README.md §5）：
 * 参数已由 `escodeCircuitMethodParamsSchemas` 严格校验，这里只做工具 op → CircuitService 方法的映射。
 * IR 的唯一所有者仍是 CircuitService；桥接层不持有状态、不缓存 IR。
 *
 * 工具失败用响应信封 `{ ok: false, error: { code, message } }` 表达：
 * `CircuitServiceError.code` 原样成为 `error.code`（规格的七个错误码），
 * 因此这条链路上 broker 与插件都只透传结果，不需要解析 JSON-RPC error.data。
 *
 * 未注册的 op 只有 `circuit_submit_recognition`（G6）：它合入时再补反向方法与 handler，
 * 现在不写占位实现，避免出现没有真实实现方的协议分支。
 */
export interface CircuitAgentBridge {
  handle<Op extends CircuitAgentToolOp>(
    op: Op,
    params: ESCodeCircuitMethodParams<Op>,
  ): Promise<CircuitToolResponse<Op>>;
}

/** 协议参数里的 workspace 身份 → CircuitService 的 CircuitWorkspaceRef（远程链路贯穿 identity 与 sessionId）。 */
function workspaceRef(params: ESCodeCircuitMethodParams<CircuitAgentToolOp>) {
  return {
    workspacePath: params.workspacePath,
    ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
    ...(params.remoteSessionId ? { remoteSessionId: params.remoteSessionId } : {}),
  };
}

/** CircuitServiceError 的 message 自带 `code: ` 前缀（domain 用它表达错误码），信封里已有 code，这里去掉重复前缀。 */
function toolErrorMessage(error: CircuitServiceError): string {
  const prefix = `${error.code}: `;
  const message = error.message.startsWith(prefix)
    ? error.message.slice(prefix.length)
    : error.message;
  return message.trim() || error.code;
}

function toToolFailure(error: unknown): { ok: false; error: { code: string; message: string } } {
  if (error instanceof CircuitServiceError)
    return { ok: false, error: { code: error.code, message: toolErrorMessage(error) } };
  // 服务实现域内抛错不是预期路径：按 io_error 报告，消息原样保留。
  return {
    ok: false,
    error: {
      code: "io_error",
      message: error instanceof Error ? error.message : String(error),
    },
  };
}

export function createCircuitAgentBridge(options: {
  getCircuitService: () => ICircuitService | undefined;
}): CircuitAgentBridge {
  const handlers: {
    [Op in CircuitAgentToolOp]: (
      service: ICircuitService,
      params: ESCodeCircuitMethodParams<Op>,
    ) => Promise<CircuitToolResponse<Op>>;
  } = {
    async parse(service, params) {
      const view = await service.parse({ ...workspaceRef(params), ...params.args });
      return { ok: true, summary: view.summary };
    },

    async get(service, params) {
      const view: CircuitIrView = await service.get({ ...workspaceRef(params), ...params.args });
      return {
        ok: true,
        summary: view.summary,
        diagnostics: view.diagnostics,
        truncated: view.truncated,
      };
    },

    async query(service, params) {
      // cursor 原样透传：内容由服务端定义，桥接层不解析、不重写（§5 分页）。
      const result = await service.query({ ...workspaceRef(params), ...params.args });
      return { ok: true, ...result };
    },

    async correct(service, params) {
      const view = await service.correct({
        ...workspaceRef(params),
        ...params.args,
        // §5：Agent 修正记录 author=agent；会话 id 落进 corrections.json 供面板追溯。
        author: { kind: "agent", sessionId: params.sessionId },
      });
      return { ok: true, summary: view.summary };
    },

    async check(service, params) {
      // §6：只读规则检查；cursor 与 query 一样原样透传，分页由服务端定义。
      const result = await service.check({ ...workspaceRef(params), ...params.args });
      return { ok: true, ...result };
    },

    async compareFirmware(service, params) {
      // §7：只读固件比对；files 是 workspace 相对路径，服务端先判格式再读文件。
      const result = await service.compareFirmware({ ...workspaceRef(params), ...params.args });
      return { ok: true, ...result };
    },

    async export(service, params) {
      const result = await service.exportSnapshot({ ...workspaceRef(params), ...params.args });
      return { ok: true, path: result.path };
    },
  };

  return {
    async handle(op, params) {
      const service = options.getCircuitService();
      // §3.5：Host 未注册 CircuitService（Web 客户端/独立的非 workspace Host）时按 unavailable 报告。
      if (!service)
        return toToolFailure(
          new CircuitServiceError("unavailable", "Circuit tools are not available on this Host"),
        ) as CircuitToolResponse<typeof op>;
      const handler = handlers[op] as (
        service: ICircuitService,
        params: ESCodeCircuitMethodParams<typeof op>,
      ) => Promise<CircuitToolResponse<typeof op>>;
      try {
        return await handler(service, params);
      } catch (error) {
        return toToolFailure(error) as CircuitToolResponse<typeof op>;
      }
    },
  };
}
