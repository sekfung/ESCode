// Manual review pending：会话 token 生成速度读数（docs/specs/conversation-generation-speed.md）。
// 证据层 L3：
// 1) 流式期间 Composer 工具条出现 live 读数（data-generation-speed-mode="live"，数值 > 0）；
// 2) 轮末读数切换为 average，数值来自 CLI 投影累计的 turnHeader.decodeTokens / decodeMs；
// 3) 读数与上下文容量同簇（输入框工具条），且不因 modelLocked 隐藏逻辑缺失。
// 本 case 需要 controlled-stream 时序：真实 replay 若一次吐完整段，live 窗口无法稳定断言；
// fixture 断言只读 Read 工具调用（buildReadonlyToolPrompt）与最终文本两个 marker。
// 窗口排空回占位符、投递缺口只清实时窗口、新轮清空保留值由单测覆盖
// （packages/ui/test/localGenerationSpeed.test.ts），不在此 spec 重复断言时序竞态。
import { clearAppData } from "../../../helpers/desktop-app.js";
import { buildReadonlyToolPrompt } from "../../../helpers/conversation-session-readonly-tool.js";
import {
  getV4GenerationSpeed,
  prepareV4ConversationE2E,
  sendV4Prompt,
  waitForV4GenerationSpeedMode,
  waitForV4TimelineContaining,
} from "../../../helpers/v4-conversation.js";

describe("会话生成速度读数", () => {
  after(async () => {
    await browser.electron.restoreAllMocks();
    await clearAppData();
  });

  it("流式中显示实时速度，轮末保留本轮解码速度", async function () {
    this.timeout(240000);
    await prepareV4ConversationE2E();

    await sendV4Prompt(buildReadonlyToolPrompt("E2E_GENERATION_SPEED_TOOL"));

    // 工具轮同样要有实时读数：读数只统计模型吐出的 token，不因工具执行停摆丢失。
    const live = await waitForV4GenerationSpeedMode("live", 60000);
    expect(Number(live.value)).toBeGreaterThan(0);
    expect(live.text).toContain("tok/s");

    await waitForV4TimelineContaining("upstream-e2e-ok", 90000);

    const average = await waitForV4GenerationSpeedMode("average", 60000);
    expect(Number(average.value)).toBeGreaterThan(0);

    // 保留态停在工具条内、与上下文容量同簇；不等于 0 也不等于空占位符。
    const settled = await getV4GenerationSpeed();
    expect(settled.mode).toBe("average");
    expect(settled.value).not.toBe("");
  });
});
