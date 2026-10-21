import type { ChildProcessWithoutNullStreams } from "node:child_process";

/** 应答 Host 的本地媒体路径授权请求；任何失败都必须回 `ok:false`，否则 Host 侧请求永远挂起。 */
export function answerAuthorize(
  child: ChildProcessWithoutNullStreams,
  request: { requestId: string; path: string },
  authorize: (path: string) => Promise<string>,
): void {
  const reply = (body: { ok: true; path: string } | { ok: false; error: string }) => {
    if (child.stdin.destroyed || !child.stdin.writable) return;
    child.stdin.write(
      `${JSON.stringify({ kind: "localMediaPreviewAuthorized", requestId: request.requestId, ...body })}\n`,
    );
  };
  void Promise.resolve()
    .then(() => authorize(request.path))
    .then(
      (path) => reply({ ok: true, path }),
      (error: unknown) =>
        reply({
          ok: false,
          error: error instanceof Error ? error.message : String(error),
        }),
    );
}

/** 应答 Host 的 Main 专属服务请求；任何失败都必须回 `ok:false`，否则 Host 侧请求永远挂起。 */
export function answerMainRequest(
  child: ChildProcessWithoutNullStreams,
  request: { requestId: string; method: string; params: unknown },
  handler: (request: { method: string; params: unknown }) => Promise<unknown>,
): void {
  const reply = (body: { ok: true; result: unknown } | { ok: false; error: string }) => {
    if (child.stdin.destroyed || !child.stdin.writable) return;
    child.stdin.write(
      `${JSON.stringify({ kind: "mainResponse", requestId: request.requestId, ...body })}\n`,
    );
  };
  void Promise.resolve()
    .then(() => handler({ method: request.method, params: request.params }))
    .then(
      (result) => reply({ ok: true, result: result ?? null }),
      (error: unknown) =>
        reply({
          ok: false,
          error: error instanceof Error ? error.message : String(error),
        }),
    );
}
