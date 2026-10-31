import type {
  RustHostHandle,
  RustHostRemoteProfileRequest,
  RustHostResolvedRemoteProfile,
} from "./rustHostBridgeTypes.js";
import { RUST_HOST_SECRET_HEADER } from "./rustHostBridgeHttp.js";

/**
 * 首连目标解析（S5-a，spec §12.2）：Host 负责 Profile 匹配/创建、显式连接与
 * `sourceServerId` 回填。失败保留结构化身份变化字段，Main 原样上抛。
 */
export async function resolveRustHostRemoteProfile(
  handle: RustHostHandle,
  request: RustHostRemoteProfileRequest,
  options?: { signal?: AbortSignal },
): Promise<RustHostResolvedRemoteProfile> {
  const response = await fetch(`http://${handle.addr}/api/source-server-resolve-profile`, {
    method: "POST",
    headers: {
      [RUST_HOST_SECRET_HEADER]: handle.secret,
      "content-type": "application/json",
    },
    body: JSON.stringify(request),
    ...(options?.signal ? { signal: options.signal } : {}),
  });
  const body = (await response.json().catch(() => undefined)) as
    | {
        error?: unknown;
        profileId?: unknown;
        sourceServerId?: unknown;
        target?: unknown;
        code?: unknown;
        pendingSourceServerId?: unknown;
        retryable?: unknown;
      }
    | undefined;
  if (!response.ok) {
    const message =
      typeof body?.error === "string" ? body.error : `HTTP ${String(response.status)}`;
    const error = new Error(
      `Rust Host refused to resolve a source server profile (HTTP ${String(response.status)}): ${message}`,
    ) as Error & {
      code?: string;
      profileId?: string;
      pendingSourceServerId?: string;
      retryable?: boolean;
    };
    if (typeof body?.code === "string") error.code = body.code;
    if (typeof body?.profileId === "string") error.profileId = body.profileId;
    if (typeof body?.pendingSourceServerId === "string") {
      error.pendingSourceServerId = body.pendingSourceServerId;
    }
    if (typeof body?.retryable === "boolean") error.retryable = body.retryable;
    throw error;
  }
  if (
    typeof body?.profileId !== "string" ||
    body.profileId.length === 0 ||
    typeof body.sourceServerId !== "string" ||
    body.sourceServerId.length === 0 ||
    !body.target ||
    typeof body.target !== "object" ||
    Array.isArray(body.target)
  ) {
    throw new Error("Rust Host profile resolution response is malformed");
  }
  return {
    profileId: body.profileId,
    sourceServerId: body.sourceServerId,
    target: body.target as Record<string, unknown>,
  };
}

/**
 * 取消在途首连（spec §12.2）：Host 按 caller key 置位，丢弃在建 transport → SSH socket 关闭。
 * 未知/已完成 key 返回 `{cancelled:false}`，因此这里不把结果当作错误。
 */
export async function cancelRustHostRemoteProfile(
  handle: RustHostHandle,
  requestId: string,
): Promise<boolean> {
  const response = await fetch(`http://${handle.addr}/api/source-server-cancel-profile`, {
    method: "POST",
    headers: {
      [RUST_HOST_SECRET_HEADER]: handle.secret,
      "content-type": "application/json",
    },
    body: JSON.stringify({ requestId }),
  });
  const body = (await response.json().catch(() => undefined)) as { cancelled?: unknown } | undefined;
  if (!response.ok) {
    throw new Error(
      `Rust Host refused to cancel the source server profile resolution (HTTP ${String(response.status)})`,
    );
  }
  return body?.cancelled === true;
}
