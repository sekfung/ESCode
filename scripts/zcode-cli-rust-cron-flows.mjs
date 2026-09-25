// docs/specs/rust-cron.md 的流程用例数据（由 generate-zcode-cli-rust-cron-corpus.mjs 使用）。
const automation = (overrides = {}) => ({
  automationId: "auto_1",
  title: " Morning digest ",
  cronExpr: "0 9 * * *",
  prompt: "Summarize",
  enabled: true,
  lifecycleStatus: "active",
  nextRunAt: 1767000000000,
  runCount: 0,
  recurring: true,
  workspacePath: "/w",
  targetTaskId: "sess_current",
  modelSelection: { providerId: "p", modelId: "m", options: { reasoningLevel: "low" } },
  mode: "autoEdit",
  scheduleRule: {
    unit: "daily",
    interval: 40,
    hour: 9,
    minute: 0,
    anchorAt: 1766000000000,
    weekdays: [1, 2],
  },
  createdAt: 1,
  updatedAt: 2,
  ...overrides,
});

export const selection = {
  providerId: "personal:fixture",
  modelId: "model-a",
  options: { reasoningLevel: "high" },
};
export const flows = [
  {
    name: "create-cron",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: " Summarize ", title: " Morning digest " },
    host: {
      "automation/checkTaskBinding": [{ result: { bound: false } }],
      "automation/create": [{ result: { automation: automation() } }],
    },
  },
  {
    name: "create-delay",
    tool: "CronCreate",
    input: { delayMinutes: 8, prompt: "class", title: "8分钟后上课" },
    mode: "auto",
    host: {
      "automation/checkTaskBinding": [{ result: { bound: false } }],
      "automation/create": [
        {
          result: {
            automation: automation({
              title: "   ",
              recurring: false,
              maxRuns: 1,
              lastRunAt: 5,
              scheduleRule: undefined,
            }),
          },
        },
      ],
    },
  },
  {
    name: "create-interval",
    tool: "CronCreate",
    input: {
      cron: "49 * * * *",
      prompt: "p",
      title: "每31小时",
      intervalUnit: "hourly",
      interval: 31,
    },
    bot: { platform: "feishu", chatId: "c1" },
    host: {
      "automation/checkTaskBinding": [{ result: { bound: false } }],
      "automation/create": [{ result: { automation: automation() } }],
    },
  },
  {
    name: "create-finite",
    tool: "CronCreate",
    input: { cron: "0 9 30 7 *", prompt: "p", title: "t", recurring: false, maxRuns: 3 },
    mode: "plan",
    host: {
      "automation/checkTaskBinding": [{ result: { bound: false } }],
      "automation/create": [{ result: { automation: automation() } }],
    },
  },
  {
    name: "create-bound",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    host: { "automation/checkTaskBinding": [{ result: { bound: true } }] },
  },
  {
    name: "create-legacy-binding",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    host: {
      "automation/checkTaskBinding": [{ error: { code: -32601, message: "Method not found" } }],
      "automation/list": [{ result: { automations: [automation({ targetTaskId: "other" })] } }],
      "automation/create": [{ result: { automation: automation() } }],
    },
  },
  {
    name: "create-legacy-bound",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    host: {
      "automation/checkTaskBinding": [{ error: { code: -32601, message: "Method not found" } }],
      "automation/list": [
        { result: { automations: [automation({ targetTaskId: "sess_current" })] } },
      ],
    },
  },
  {
    name: "create-binding-failure",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    host: { "automation/checkTaskBinding": [{ error: { code: -32603, message: "db down" } }] },
  },
  {
    name: "create-limit",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    host: {
      "automation/checkTaskBinding": [{ result: { bound: false } }],
      "automation/create": [
        {
          error: {
            code: -32603,
            message: "AUTOMATION_CREATE_LIMIT_REACHED: at most 20 tasks; delete one first",
          },
        },
      ],
    },
  },
  {
    name: "create-host-error",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    host: {
      "automation/checkTaskBinding": [{ result: { bound: false } }],
      "automation/create": [{ error: { code: -32603, message: "invalid cron" } }],
    },
  },
  {
    name: "create-in-automation-run",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    activeAutomationId: "auto_9",
    host: {},
  },
  {
    name: "create-automation-turn",
    tool: "CronCreate",
    input: { cron: "0 9 * * *", prompt: "p", title: "t" },
    automationTurn: true,
    host: {},
  },
  {
    name: "update-interval",
    tool: "CronUpdate",
    input: {
      id: " auto_1 ",
      title: " 每40天 ",
      intervalUnit: "daily",
      interval: 40,
      cron: "0 9 * * *",
    },
    host: { "automation/update": [{ result: { automation: automation() } }] },
  },
  {
    name: "update-finite",
    tool: "CronUpdate",
    input: { id: "auto_1", title: "t", recurring: false, maxRuns: 3, prompt: "np" },
    host: {
      "automation/update": [
        { result: { automation: automation({ recurring: false, maxRuns: 3 }) } },
      ],
    },
  },
  {
    name: "update-clear",
    tool: "CronUpdate",
    input: { id: "auto_1", title: "t", recurring: true, maxRuns: null },
    host: { "automation/update": [{ result: { automation: automation() } }] },
  },
  {
    name: "update-automation-turn",
    tool: "CronUpdate",
    input: { id: "auto_1", title: "t" },
    automationTurn: true,
    host: {},
  },
  {
    name: "list",
    tool: "CronList",
    input: {},
    host: {
      "automation/list": [
        {
          result: {
            automations: [
              automation(),
              automation({
                automationId: "auto_2",
                enabled: false,
                lifecycleStatus: "paused",
                scheduleRule: undefined,
                lastRunAt: 9,
              }),
            ],
          },
        },
      ],
    },
  },
  {
    name: "list-empty",
    tool: "CronList",
    input: {},
    automationTurn: true,
    host: { "automation/list": [{ result: { automations: [] } }] },
  },
  {
    name: "delete-found",
    tool: "CronDelete",
    input: { id: " auto_1 " },
    host: { "automation/delete": [{ result: { deleted: true } }] },
  },
  {
    name: "delete-missing",
    tool: "CronDelete",
    input: { id: "auto_x" },
    host: { "automation/delete": [{ result: { deleted: false } }] },
  },
  {
    name: "delete-automation-turn",
    tool: "CronDelete",
    input: { id: "auto_1" },
    automationTurn: true,
    host: {},
  },
];
