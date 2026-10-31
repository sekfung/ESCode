import { describe, expect, it } from "vitest";
import { hostResponseMessageSchema, providerProvisioningResultSchema } from "@escode/shared";

describe("Provisioning 安全错误码契约", () => {
  it.each([undefined, "target-refresh-failed"])("兼容结果的可选错误码 %s", (errorCode) => {
    const result = {
      syncId: "fixture",
      status: "failed",
      personalProviderCount: 0,
      credentialCount: 0,
      rolledBack: true,
      ...(errorCode ? { errorCode } : {}),
    };
    expect(providerProvisioningResultSchema.safeParse(result).success).toBe(true);
    expect(
      providerProvisioningResultSchema.safeParse({ ...result, errorCode: "token=fixture-secret" })
        .success,
    ).toBe(false);
  });

  it("Host 消息面不再接受 ProviderProvisioningExecute 的执行结果", () => {
    // spec §12.1/§12.5：per-window Node Host 的 ProviderProvisioningExecute 已删除（S5-f-5），
    // 远程 provisioning 只由 Rust Host 自己执行，不再有 Host → Main 的结果回传消息。
    const hostResult = {
      type: "provider-provisioning-execution-result",
      requestId: "fixture",
      environmentKey: "fixture",
      status: "failed",
    };
    expect(hostResponseMessageSchema.safeParse(hostResult).success).toBe(false);
  });
});
