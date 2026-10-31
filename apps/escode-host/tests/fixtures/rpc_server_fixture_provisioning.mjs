// `provider-provisioning-target`（S4-g）：Host 私有远端能力。默认注册为 applied；
// 环境变量可切换失败结果或完全缺席（模拟旧 daemon 不广播 channel）。
import { appendFileSync } from "node:fs";

export function registerProvisioningChannel(server) {
  if (process.env.ESCODE_FIXTURE_PROVISIONING_CHANNEL === "0") return;
  const capture = process.env.ESCODE_FIXTURE_PROVISIONING_CAPTURE;
  const status = process.env.ESCODE_FIXTURE_PROVISIONING_STATUS ?? "applied";
  const statusSequence = (
    process.env.ESCODE_FIXTURE_PROVISIONING_STATUS_SEQUENCE ?? ""
  )
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
  const errorCode = process.env.ESCODE_FIXTURE_PROVISIONING_ERROR_CODE;
  const errorMessage = process.env.ESCODE_FIXTURE_PROVISIONING_ERROR_MESSAGE;
  let calls = 0;
  server.registerChannel("provider-provisioning-target", {
    call: async (_ctx, command, rawArg) => {
      if (command !== "apply") throw new Error(`unknown command ${command}`);
      // 真实 TS `ProxyChannel` 用 `apply(handler, args)` 展开实参，信封必须是单元素数组；
      // fixture 与线上契约保持一致，裸对象直接判错而不是悄悄兼容。
      const envelope = Array.isArray(rawArg) ? rawArg[0] : undefined;
      if (!envelope || typeof envelope !== "object") {
        throw new Error("provider-provisioning-target.apply expects a single envelope argument");
      }
      if (capture) appendFileSync(capture, `${JSON.stringify(envelope)}\n`);
      const providerRules =
        envelope?.personalConfig?.providerConfigRules?.providerRules;
      const resultStatus =
        statusSequence[Math.min(calls, statusSequence.length - 1)] ?? status;
      calls += 1;
      return {
        syncId: envelope.syncId,
        status: resultStatus,
        personalProviderCount: Array.isArray(providerRules)
          ? providerRules.length
          : 0,
        credentialCount: Array.isArray(envelope.credentials)
          ? envelope.credentials.length
          : 0,
        ...(errorCode ? { errorCode } : {}),
        ...(errorMessage ? { errorMessage } : {}),
        rolledBack:
          resultStatus === "failed" || resultStatus === "rollback_failed",
      };
    },
    listen: () => () => ({ dispose: () => undefined }),
  });
}
