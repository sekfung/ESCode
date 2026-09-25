import type { ZCodeProtocolClient } from "../src/zcode-agent/zcodeProtocolClient.js";

type Message = Record<string, unknown>;

export interface HostRequestResponder {
  readonly client: ZCodeProtocolClient;
  hostHandlers: Record<string, (params: any) => unknown>;
  readonly hostRequests: { method: string; params: unknown }[];
  integratedTerminalShell?: Message;
  readonly runtimePreferenceRequests: unknown[];
}

/**
 * 模拟 Host 应答 runtime 反向请求：用例脚本化的方法（如 automation/*）记录后按脚本应答，
 * 结果为 {error} 时回错误；运行时偏好与真实 Host（zcodeAgentService 的 onRequest）一致，
 * 不应答时 Rust 首个 Bash 会等满 15s 超时。
 */
export function answerHostRequest(
  harness: HostRequestResponder,
  request: { id: string | number; method: string; params?: unknown },
): void {
  const handler = harness.hostHandlers[request.method];
  if (handler) {
    harness.hostRequests.push({ method: request.method, params: request.params });
    void Promise.resolve(handler(request.params)).then((result: any) =>
      result && typeof result === "object" && "error" in result
        ? harness.client.respondError(request.id, result.error)
        : harness.client.respond(request.id, result),
    );
    return;
  }
  if (request.method !== "session/requestRuntimePreferences") return;
  harness.runtimePreferenceRequests.push(request.params);
  void harness.client.respond(request.id, {
    nativeSearchEnhancementsEnabled: true,
    ...(harness.integratedTerminalShell
      ? { integratedTerminalShell: harness.integratedTerminalShell }
      : {}),
  });
}
