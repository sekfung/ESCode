import type { CircuitErrorCode } from "@escode/shared/circuit";

/**
 * 服务层错误：错误码与 `@escode/shared/circuit` 的工具契约一致（§5），
 * 由 Host 侧的桥接层原样映射成工具失败响应。
 */
export class CircuitServiceError extends Error {
  constructor(
    readonly code: CircuitErrorCode,
    message: string,
  ) {
    super(`${code}: ${message}`);
    this.name = "CircuitServiceError";
  }
}

export function isCircuitServiceError(error: unknown): error is CircuitServiceError {
  return error instanceof CircuitServiceError;
}

/** 解析器与文档层用消息前缀表达错误码（例如 `unsupported_format: …`）。 */
const CODE_PREFIX = /^(unavailable|not_found|unsupported_format|invalid_ir|stale_revision|too_large|io_error): ?/u;

export function toCircuitServiceError(error: unknown, fallbackCode: CircuitErrorCode): never {
  if (isCircuitServiceError(error)) throw error;
  const message = error instanceof Error ? error.message : String(error);
  const matched = CODE_PREFIX.exec(message);
  if (matched) throw new CircuitServiceError(matched[1] as CircuitErrorCode, message.slice(matched[0].length));
  throw new CircuitServiceError(fallbackCode, message);
}

/** 文件系统错误统一映射（ENOENT → not_found，其余 → io_error）。 */
export function mapIoError(error: unknown, path: string): never {
  const code = (error as { code?: string } | null)?.code;
  if (code === "ENOENT") throw new CircuitServiceError("not_found", `文件不存在：${path}`);
  toCircuitServiceError(error, "io_error");
}
