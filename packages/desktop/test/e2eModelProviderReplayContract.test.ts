import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// 套餐族删除（2026-10-15）：coding-plan mock 的启动契约测试（shouldPrepareCodingPlanTeamMock）
// 随 mock helper 一并移除；本文件只保留上游回放夹具的传输契约校验。

interface ReplayFixtureFile {
  fixtures: Array<{
    id: string;
    match: { pathIncludes?: string };
    response: { body?: string; text?: unknown };
  }>;
}

function readReplayFixture(relativePath: string): ReplayFixtureFile {
  return JSON.parse(
    readFileSync(new URL(relativePath, import.meta.url), "utf8"),
  ) as ReplayFixtureFile;
}

describe("model provider E2E replay contract", () => {
  it("covers both seeded OpenAI and startup Anthropic transports for restart cases", () => {
    const restart = readReplayFixture(
      "./e2e/fixtures/upstream/conversation-session/conversation-session-model-provider-restart-recovery.json",
    );
    const baseFixtureIds = ["model-provider-restart-title", "model-provider-restart-r01"];

    for (const id of baseFixtureIds) {
      const openai = restart.fixtures.find((fixture) => fixture.id === id);
      const anthropic = restart.fixtures.find((fixture) => fixture.id === `${id}-anthropic`);
      expect(openai?.match.pathIncludes).toBe("/chat/completions");
      expect(openai?.response.body).toBeDefined();
      expect(anthropic?.match.pathIncludes).toBe("/messages");
      expect(anthropic?.response.text).toBeDefined();
    }
  });
});
