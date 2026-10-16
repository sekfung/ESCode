// 会话常驻 Selection 的恢复回归。
// 恢复规则：entry 里的完整选型原样恢复；推理档位非法时按 providerId/modelId 兜底，
// 不借旧 thoughtLevel/消息/default 补值（执行前仍由 Registry 严格校验）。
import { describe, expect, it } from "vitest";
import {
  SESSION_ENTRY_MODEL_SELECTION,
  type SessionEntryInfo,
  type SessionId,
  type SessionStorePort,
} from "@escode/contracts";
import { readSessionModelSelection } from "../src/app/session-store.js";

const SESSION_ID = "sess_restore" as SessionId;

function storeWithEntryData(data: unknown): Pick<SessionStorePort, "sessionEntries"> {
  const entry = {
    id: `${SESSION_ID}:runtime-model-selection`,
    sessionID: SESSION_ID,
    type: SESSION_ENTRY_MODEL_SELECTION,
    time: { created: 1, updated: 1 },
    data,
  } as SessionEntryInfo;
  return { sessionEntries: async () => [entry] } as Pick<SessionStorePort, "sessionEntries">;
}

describe("恢复会话常驻 Selection", () => {
  it("普通选型原样恢复", async () => {
    await expect(
      readSessionModelSelection(
        storeWithEntryData({
          providerId: "account:bigmodel-team-coding-plan",
          modelId: "GLM-5.3",
          options: { reasoningLevel: "max" },
        }),
        SESSION_ID,
      ),
    ).resolves.toEqual({
      providerId: "account:bigmodel-team-coding-plan",
      modelId: "GLM-5.3",
      options: { reasoningLevel: "max" },
    });
  });

  it("档位非法时按模型身份兜底恢复", async () => {
    // 兜底分支只捞 providerId/modelId：非法 reasoning 不能抹掉可恢复的模型身份。
    await expect(
      readSessionModelSelection(
        storeWithEntryData({
          providerId: "account:bigmodel-team-coding-plan",
          modelId: "GLM-5.3",
          options: { reasoningLevel: 3 },
        }),
        SESSION_ID,
      ),
    ).resolves.toEqual({ providerId: "account:bigmodel-team-coding-plan", modelId: "GLM-5.3" });
  });

  it("没有 entry 时返回空", async () => {
    await expect(
      readSessionModelSelection({ sessionEntries: async () => [] }, SESSION_ID),
    ).resolves.toBeUndefined();
    await expect(readSessionModelSelection({}, SESSION_ID)).resolves.toBeUndefined();
  });
});
