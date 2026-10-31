// `escode-agent` 的 runtime preferences 最小 owner（S4-g）：注册后主动派发一次动态请求，
// 记录 Host 的 `respondSessionRuntimePreferences` 应答。非该测试不注册，避免影响既有 fixture。
import { appendFileSync } from "node:fs";

import { Emitter } from "../../../../packages/rpc/src/foundation.ts";

const EVENT = "onDynamicSessionRuntimePreferencesRequest";

export function registerRuntimePreferencesChannel(server) {
  if (process.env.ESCODE_FIXTURE_RUNTIME_PREFERENCES !== "1") return;
  const capture = process.env.ESCODE_FIXTURE_RUNTIME_PREFERENCES_CAPTURE;
  const intervalMs = Number.parseInt(
    process.env.ESCODE_FIXTURE_RUNTIME_PREFERENCES_INTERVAL_MS ?? "0",
    10,
  );
  const request = {
    requestId: "preferences-request-1",
    sessionId: "preferences-session-1",
    scope: "runtime-materialization",
  };
  const requests = new Emitter();

  server.registerChannel("escode-agent", {
    call: async (_ctx, command, rawArg) => {
      if (command !== "respondSessionRuntimePreferences") {
        throw new Error(`unknown command ${command}`);
      }
      // 与真实 `ProxyChannel` 一致：handler 以 `apply(handler, args)` 展开，信封必须是单元素数组。
      const envelope = Array.isArray(rawArg) ? rawArg[0] : undefined;
      if (!envelope || typeof envelope !== "object") {
        throw new Error(
          "respondSessionRuntimePreferences expects a single response argument",
        );
      }
      if (capture) appendFileSync(capture, `${JSON.stringify(envelope)}\n`);
      return null;
    },
    listen: (_ctx, event) => {
      if (event !== EVENT) throw new Error(`unknown event ${event}`);
      return (listener) => {
        const subscription = requests.event(listener);
        const fire = () => requests.fire(request);
        if (intervalMs > 0) {
          const timer = setInterval(fire, intervalMs);
          return {
            dispose: () => {
              subscription.dispose();
              clearInterval(timer);
            },
          };
        }
        const timer = setTimeout(fire, 0);
        return {
          dispose: () => {
            subscription.dispose();
            clearTimeout(timer);
          },
        };
      };
    },
  });
}
