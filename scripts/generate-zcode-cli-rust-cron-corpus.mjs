// Run with node --import tsx. Cron 工具的 TS oracle（docs/specs/rust-cron.md）：
// 真实 handler + 真实 createProtocolAutomationPort，假 Host 按脚本应答并记录每个反向请求。
// 覆盖入参校验（含 trim 后的解析结果）、Host 请求参数、输出与模型可见文案、标题冻结与各类守卫。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import {
  CronCreateInputSchema,
  CronDeleteInputSchema,
  CronListInputSchema,
  CronUpdateInputSchema,
  isAutomationCreateLimitError,
} from "../apps/zcode-cli/packages/contracts/src/index.ts";
import {
  cronCreateToolEntry,
  cronDeleteToolEntry,
  cronListToolEntry,
  cronUpdateToolEntry,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/cron.ts";
import { createProtocolAutomationPort } from "../apps/zcode-cli/packages/bootstrap/src/zcode-protocol/automation-port.ts";
import { ProtocolRequestError } from "../apps/zcode-cli/packages/bootstrap/src/zcode-protocol/server-types.ts";
import { flows, selection } from "./zcode-cli-rust-cron-flows.mjs";

const schemas = {
  CronCreate: CronCreateInputSchema,
  CronUpdate: CronUpdateInputSchema,
  CronDelete: CronDeleteInputSchema,
  CronList: CronListInputSchema,
};
const entries = {
  CronCreate: cronCreateToolEntry,
  CronUpdate: cronUpdateToolEntry,
  CronDelete: cronDeleteToolEntry,
  CronList: cronListToolEntry,
};

const validation = [
  ["CronCreate", { cron: " */20 * * * * ", prompt: " drink water ", title: " 每20分钟喝水 " }],
  ["CronCreate", { delayMinutes: 8, prompt: "class", title: "8分钟后上课" }],
  ["CronCreate", { delayMinutes: 8, cron: "* * * * *", prompt: "p", title: "t" }],
  ["CronCreate", { delayMinutes: 8, recurring: true, prompt: "p", title: "t" }],
  ["CronCreate", { delayMinutes: 8, maxRuns: 2, prompt: "p", title: "t" }],
  ["CronCreate", { delayMinutes: null, cron: "0 9 * * 1-5", prompt: "p", title: "t" }],
  ["CronCreate", { delayMinutes: 0, prompt: "p", title: "t" }],
  ["CronCreate", { delayMinutes: 525601, prompt: "p", title: "t" }],
  ["CronCreate", { delayMinutes: 1.5, prompt: "p", title: "t" }],
  ["CronCreate", { prompt: "p", title: "t" }],
  ["CronCreate", { cron: "   ", prompt: "p", title: "t" }],
  [
    "CronCreate",
    { cron: "* * * * *", prompt: "p", title: "t", intervalUnit: "daily", interval: 40 },
  ],
  ["CronCreate", { cron: "* * * * *", prompt: "p", title: "t", intervalUnit: "daily" }],
  [
    "CronCreate",
    { cron: "* * * * *", prompt: "p", title: "t", intervalUnit: "daily", interval: 201 },
  ],
  [
    "CronCreate",
    {
      cron: "* * * * *",
      prompt: "p",
      title: "t",
      intervalUnit: "daily",
      interval: 4,
      recurring: false,
    },
  ],
  [
    "CronCreate",
    { cron: "* * * * *", prompt: "p", title: "t", intervalUnit: "daily", interval: 4, maxRuns: 3 },
  ],
  [
    "CronCreate",
    { cron: "* * * * *", prompt: "p", title: "t", intervalUnit: "fortnightly", interval: 4 },
  ],
  ["CronCreate", { delayMinutes: 5, prompt: "p", title: "t", intervalUnit: "daily", interval: 4 }],
  ["CronCreate", { cron: "0 9 30 7 *", prompt: "p", title: "t", recurring: false, maxRuns: 3 }],
  ["CronCreate", { cron: "0 9 * * *", prompt: "p", title: "t", extra: 1 }],
  ["CronCreate", { cron: "0 9 * * *", prompt: "", title: "t" }],
  ["CronUpdate", { id: "a1", title: "new" }],
  ["CronUpdate", { id: "a1" }],
  ["CronUpdate", { id: " a1 ", title: " t ", cron: " 0 8 * * * " }],
  ["CronUpdate", { id: "a1", title: "t", maxRuns: null }],
  ["CronUpdate", { id: "a1", title: "t", maxRuns: null, recurring: true }],
  ["CronUpdate", { id: "a1", title: "t", recurring: true, maxRuns: 3 }],
  ["CronUpdate", { id: "a1", title: "t", recurring: false, maxRuns: 3 }],
  ["CronUpdate", { id: "a1", title: "t", intervalUnit: "hourly", interval: 31 }],
  ["CronUpdate", { id: "a1", title: "t", intervalUnit: "hourly" }],
  ["CronUpdate", { id: "a1", title: "t", intervalUnit: "hourly", interval: 3, recurring: false }],
  ["CronUpdate", { id: "a1", title: "t", intervalUnit: "hourly", interval: 3, maxRuns: 2 }],
  [
    "CronUpdate",
    { id: "a1", title: "t", intervalUnit: "hourly", interval: 3, maxRuns: null, recurring: true },
  ],
  ["CronUpdate", { id: "", title: "t" }],
  ["CronUpdate", { id: "a1", title: "t", bogus: true }],
  ["CronDelete", { id: " a1 " }],
  ["CronDelete", { id: "" }],
  ["CronDelete", {}],
  ["CronList", {}],
  ["CronList", { x: 1 }],
];
const validationCases = validation.map(([tool, input]) => {
  const result = schemas[tool].safeParse(input);
  return { tool, input, ok: result.success, ...(result.success ? { data: result.data } : {}) };
});

const flowCases = [];
for (const flow of flows) {
  const requests = [];
  const titles = [];
  const script = structuredClone(flow.host);
  const record = {
    activeAutomationId: flow.activeAutomationId,
    activeBotDeliveryTarget: flow.bot,
    traceContext: {},
    app: {
      sessionId: "sess_current",
      runtime: { getSessionModelSelection: () => selection },
      getMode: () => flow.mode ?? "build",
      async setCustomSessionTitle({ title }) {
        titles.push(title);
      },
    },
  };
  const context = {
    sessions: new Map(),
    logger: undefined,
    async requestClient(method, params) {
      requests.push({ method, params: JSON.parse(JSON.stringify(params)) });
      const next = script[method]?.shift();
      if (!next) throw new Error(`unscripted ${method}`);
      if (next.error) throw new ProtocolRequestError(next.error.code, next.error.message);
      return structuredClone(next.result);
    },
  };
  const port = createProtocolAutomationPort(context, () => record);
  let output;
  let error;
  let limit = false;
  try {
    output = await entries[flow.tool].handler(flow.input, {
      toolCallId: "t",
      sessionId: "sess_current",
      automationTurn: flow.automationTurn,
      automationPort: port,
      model: { providerId: "personal:fixture", modelId: "model-a" },
    });
  } catch (caught) {
    error = caught instanceof Error ? caught.message : String(caught);
    limit = isAutomationCreateLimitError(caught);
  }
  flowCases.push({
    name: flow.name,
    tool: flow.tool,
    input: flow.input,
    automationTurn: flow.automationTurn === true,
    activeAutomationId: flow.activeAutomationId ?? null,
    bot: flow.bot ?? null,
    mode: flow.mode ?? "build",
    selection,
    host: flow.host,
    requests,
    titles,
    ...(output !== undefined ? { output, modelContent: JSON.stringify(output) } : { error, limit }),
  });
}

const content = `${JSON.stringify({ validationCases, flowCases })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/cron_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust cron corpus differs from TS");
} else await writeFile(target, content);
