// Run with node --import tsx. OffPeak 工具的 TS oracle（docs/specs/rust-offpeak.md 第一期）：
// 真实 handler + 真实 createProtocolOffPeakPort，假 Host 按脚本应答并记录每个反向请求。
// 覆盖入参、会话绑定、递归拒绝、fail-closed、各失败分类与成功输出。--check 防漂移。
import { readFile, writeFile } from "node:fs/promises";
import {
  offPeakCreateToolEntry,
  offPeakListToolEntry,
} from "../apps/escode-cli/packages/core/src/tool/handlers/off-peak.ts";
import { createProtocolOffPeakPort } from "../apps/escode-cli/packages/bootstrap/src/escode-protocol/offpeak-port.ts";
import { ProtocolRequestError } from "../apps/escode-cli/packages/bootstrap/src/escode-protocol/server-types.ts";

const task = (overrides = {}) => ({
  offPeakTaskId: "offpeak-1",
  title: "Refactor utils",
  status: "queued",
  queuePosition: 3,
  sessionId: "sess_current",
  createdAt: 1767000000000,
  ...overrides,
});
const failure = (errorCategory, errorCode, failureStage = "ticket_request") => ({
  ok: false,
  failureStage,
  errorCategory,
  errorCode,
});
const unbound = { result: { tasks: [task({ sessionId: "sess_other" })] } };
const create = { title: "Refactor utils", prompt: "Refactor the utils directory" };

const flows = [
  {
    name: "create-queued",
    tool: "OffPeakCreate",
    input: create,
    host: { "offPeak/list": [unbound], "offPeak/create": [{ result: { ok: true, task: task() } }] },
  },
  {
    name: "create-no-position-with-options",
    tool: "OffPeakCreate",
    input: {
      title: "  Nightly cleanup ",
      prompt: " Clean up ",
      permissionMode: "build",
      model: " glm-5 ",
      thoughtLevel: "low",
    },
    host: {
      "offPeak/list": [{ result: { tasks: [] } }],
      "offPeak/create": [
        {
          result: {
            ok: true,
            task: {
              offPeakTaskId: "offpeak-2",
              title: "Nightly cleanup",
              status: "queued",
              createdAt: 5,
            },
          },
        },
      ],
    },
  },
  {
    name: "bound-terminal-allows-create",
    tool: "OffPeakCreate",
    input: create,
    host: {
      "offPeak/list": [
        { result: { tasks: [task({ status: "completed" }), task({ status: "cancelled" })] } },
      ],
      "offPeak/create": [{ result: { ok: true, task: task({ queuePosition: 1 }) } }],
    },
  },
  {
    name: "bound-pending-rejects",
    tool: "OffPeakCreate",
    input: create,
    host: { "offPeak/list": [{ result: { tasks: [task({ status: "paused" })] } }] },
  },
  {
    name: "bound-check-fails-closed",
    tool: "OffPeakCreate",
    input: create,
    host: { "offPeak/list": [{ error: { code: -32603, message: "db down" } }] },
  },
  {
    name: "recursion-from-offpeak-run",
    tool: "OffPeakCreate",
    input: create,
    activeOffPeakTaskId: "offpeak-9",
    host: {},
  },
  {
    name: "offpeak-turn-denied",
    tool: "OffPeakCreate",
    input: create,
    offPeakTurn: true,
    host: {},
  },
  {
    name: "whitespace-title",
    tool: "OffPeakCreate",
    input: { title: "   ", prompt: "p" },
    host: {},
  },
  ...[
    ["quota", failure("quota_3103", "3103")],
    ["eligibility", failure("eligibility_3101", "3101")],
    ["model-not-allowed", failure("client_validation", "model_not_allowed", "client_validation")],
    ["session-bound", failure("client_validation", "session_bound", "client_validation")],
    ["disabled", failure("client_validation", "offpeak_disabled", "client_validation")],
    ["other-validation", failure("client_validation", "bad_title", "client_validation")],
    ["network", failure("network", "ECONNRESET")],
    ["invalid-response", failure("invalid_response", "bad_json")],
    ["local-persist", failure("local_persist", "sqlite", "local_persist")],
    ["unknown", failure("unknown", "x")],
  ].map(([name, result]) => ({
    name: `create-failure-${name}`,
    tool: "OffPeakCreate",
    input: create,
    host: { "offPeak/list": [unbound], "offPeak/create": [{ result }] },
  })),
  {
    name: "create-host-error",
    tool: "OffPeakCreate",
    input: create,
    host: {
      "offPeak/list": [unbound],
      "offPeak/create": [{ error: { code: -32601, message: "Method not found: offPeak/create" } }],
    },
  },
  {
    name: "list",
    tool: "OffPeakList",
    input: {},
    host: {
      "offPeak/list": [
        {
          result: {
            tasks: [
              task(),
              task({
                offPeakTaskId: "offpeak-3",
                status: "running",
                queuePosition: undefined,
                sessionId: undefined,
              }),
            ],
          },
        },
      ],
    },
  },
  {
    name: "list-host-error",
    tool: "OffPeakList",
    input: {},
    host: { "offPeak/list": [{ error: { code: -32603, message: "list failed" } }] },
  },
];

const entries = { OffPeakCreate: offPeakCreateToolEntry, OffPeakList: offPeakListToolEntry };
const cases = [];
for (const flow of flows) {
  const requests = [];
  const script = structuredClone(flow.host);
  const record = {
    activeOffPeakTaskId: flow.activeOffPeakTaskId,
    app: { sessionId: "sess_current" },
  };
  const context = {
    sessions: new Map(),
    logger: undefined,
    async requestClient(method, params, schema) {
      requests.push({ method, params: JSON.parse(JSON.stringify(params)) });
      const next = script[method]?.shift();
      if (!next) throw new Error(`unscripted ${method}`);
      if (next.error) throw new ProtocolRequestError(next.error.code, next.error.message);
      return schema.parse(structuredClone(next.result));
    },
  };
  const port = createProtocolOffPeakPort(context, () => record);
  let output;
  let error;
  try {
    output = await entries[flow.tool].handler(flow.input, {
      toolCallId: "t",
      sessionId: "sess_current",
      offPeakTurn: flow.offPeakTurn,
      offPeakPort: port,
    });
  } catch (caught) {
    error = caught instanceof Error ? caught.message : String(caught);
  }
  cases.push({
    name: flow.name,
    tool: flow.tool,
    input: flow.input,
    offPeakTurn: flow.offPeakTurn === true,
    activeOffPeakTaskId: flow.activeOffPeakTaskId ?? null,
    host: JSON.parse(JSON.stringify(flow.host)),
    requests,
    ...(output !== undefined ? { output, modelContent: JSON.stringify(output) } : { error }),
  });
}

const content = `${JSON.stringify({ cases })}\n`;
const target = new URL(
  "../apps/escode-cli-rust/crates/domain/tests/fixtures/offpeak_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust offpeak corpus differs from TS");
} else await writeFile(target, content);
