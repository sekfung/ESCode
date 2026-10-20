import { describe, expect, it } from "vitest";
import enUS from "@/i18n/locales/en-US.js";
import zhCN from "@/i18n/locales/zh-CN.js";

describe("Automations terminology", () => {
  it("localizes the top-level area as Automations / 自动化", () => {
    expect(enUS["workspace.openScheduledSettings"]).toBe("Automations");
    expect(enUS["settings.automations.title"]).toBe("Automations");
    expect(zhCN["workspace.openScheduledSettings"]).toBe("自动化");
    expect(zhCN["settings.automations.title"]).toBe("自动化");
  });

  it("keeps 定时任务 for the scheduled-task subtype", () => {
    expect(zhCN["automations.list.title"]).toBe("定时任务");
    expect(zhCN["taskList.cronTaskLabel"]).toBe("定时任务");
  });

  it("keeps the empty-state description focused on scheduled task creation", () => {
    expect(enUS["automations.description"]).toBe("Schedule recurring tasks to run automatically.");
    expect(zhCN["automations.description"]).toBe("创建定时任务，按计划自动运行。");
  });

  it("keeps permission labels on the main conversation vocabulary", () => {
    const modeValues = ["build", "edit", "plan", "yolo"] as const;

    expect(modeValues.map((value) => zhCN[`mode.label.glm.${value}`])).toEqual([
      "变更前确认",
      "自动编辑",
      "计划模式",
      "完全访问",
    ]);
    expect(modeValues.map((value) => enUS[`mode.label.glm.${value}`])).toEqual([
      "Ask before changes",
      "Edit automatically",
      "Plan mode",
      "Full access",
    ]);
  });

  it("does not retain locale entries for deleted static template catalogs", () => {
    const removedPrefixes = ["automations.template."];

    for (const messages of [enUS, zhCN]) {
      expect(
        Object.keys(messages).some((key) =>
          removedPrefixes.some((prefix) => key.startsWith(prefix)),
        ),
      ).toBe(false);
    }
  });

  it("uses action labels for scheduled task creation", () => {
    expect(enUS["automations.createManually"]).toBe("Create scheduled task");
    expect(zhCN["automations.createManually"]).toBe("创建定时任务");
    expect(enUS["automations.createViaChat"]).toBe("Create in chat");
    expect(zhCN["automations.createViaChat"]).toBe("去会话中创建");
  });

  it("keeps imminent next-run copy grammatical across locales", () => {
    const englishNextRun = enUS["automations.nextRun"].replace(
      "{when}",
      enUS["automations.time.soon"],
    );
    const chineseNextRun = zhCN["automations.nextRun"].replace(
      "{when}",
      zhCN["automations.time.soon"],
    );

    expect(englishNextRun).toBe("Next run soon");
    expect(chineseNextRun).toBe("下次运行 1 分钟内");
  });

  it("uses productized discard-draft copy for scheduled tasks", () => {
    expect(enUS["automations.unsaved.title"]).toBe("Discard scheduled task draft?");
    expect(enUS["automations.unsaved.description"]).toBe(
      "Your changes to this scheduled task will be lost",
    );
    expect(zhCN["automations.unsaved.title"]).toBe("丢弃定时任务的草稿？");
    expect(zhCN["automations.unsaved.description"]).toBe("你对这个定时任务的更改将会丢失");
    expect(zhCN["automations.unsaved.discard"]).toBe("丢弃");
  });
});
