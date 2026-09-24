// Run with node --import tsx. Input schemas are generated from current TS contracts.
import { readFile, writeFile } from "node:fs/promises";
import { sep } from "node:path";
import { format } from "oxfmt";
import { askUserQuestionToolEntry } from "../apps/zcode-cli/packages/core/src/tool/handlers/ask-user-question.ts";
import { skillToolEntry } from "../apps/zcode-cli/packages/core/src/tool/handlers/skill.ts";
import { createAgentToolEntry } from "../apps/zcode-cli/packages/core/src/tool/handlers/agent.ts";
import { sendMessageToolEntry } from "../apps/zcode-cli/packages/core/src/tool/handlers/send-message.ts";
import { builtInTools } from "../apps/zcode-cli/packages/core/src/tool/handlers/index.ts";
import { normalizeAgentProfiles } from "../apps/zcode-cli/packages/core/src/subagent/profile.ts";
import { buildExploreAgentPrompt } from "../apps/zcode-cli/packages/core/src/subagent/explore.ts";
import { buildSubagentCommonNotes } from "../apps/zcode-cli/packages/core/src/subagent/system-prompt.ts";
import { buildPersistentAgentMemoryPrompt } from "../apps/zcode-cli/packages/core/src/subagent/persistent-memory-prompt.ts";
import { DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS } from "../apps/zcode-cli/packages/bootstrap/src/app/official-plugin-definitions.ts";
import {
  enterPlanModeToolEntry,
  exitPlanModeToolEntry,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/plan-mode.ts";
import {
  buildPlanModeExitReminderBody,
  buildRuntimeModeReminderBody,
} from "../apps/zcode-cli/packages/core/src/runtime/helpers/runtime-reminders.ts";
import {
  todoReadToolEntry,
  todoWriteToolEntry,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/todo.ts";
const tools = [
  ["Agent", "agent", "AgentInputJsonSchema"],
  ["SendMessage", "send-message", "SendMessageInputJsonSchema"],
  ["Skill", "skill", "SkillInputJsonSchema"],
  ["TodoRead", "todo", "TodoReadInputJsonSchema"],
  ["TodoWrite", "todo", "TodoWriteInputJsonSchema"],
  ["Read", "read", "ReadInputJsonSchema"],
  ["Write", "write", "WriteInputJsonSchema"],
  ["Edit", "edit", "EditInputJsonSchema"],
  ["Glob", "glob", "GlobInputJsonSchema"],
  ["Grep", "grep", "GrepInputJsonSchema"],
  ["Bash", "bash", "BashInputJsonSchema"],
  ["TaskOutput", "task-output", "TaskOutputInputJsonSchema"],
  ["TaskStop", "task-stop", "TaskStopInputJsonSchema"],
  ["AskUserQuestion", "ask-user-question", "AskUserQuestionInputJsonSchema"],
  ["EnterPlanMode", "plan-mode", "EnterPlanModeInputJsonSchema"],
  ["ExitPlanMode", "plan-mode", "ExitPlanModeInputJsonSchema"],
];
// plan 模式 reminder：直接调用 TS 的节奏函数取完整版（首次）与精简版（第 2 次、间隔 5 个真实用户轮次后）。
const realUser = { message: { role: "user", content: "u" }, metadata: { source: "real_user" } };
const modeReminder = { message: { role: "user", content: "r" }, metadata: { source: "runtime_mode" } };
const planModeReminders = {
  full: buildRuntimeModeReminderBody([], "build", true),
  sparse: buildRuntimeModeReminderBody([modeReminder, ...Array(5).fill(realUser)], "build", true),
  exit: buildPlanModeExitReminderBody(),
};
if (!planModeReminders.full || !planModeReminders.sparse || planModeReminders.full === planModeReminders.sparse)
  throw new Error("Unexpected TS plan mode reminder cadence");
const schemas = {};
for (const [name, file, key] of tools) {
  schemas[name] = (await import(`../apps/zcode-cli/packages/contracts/src/tools/${file}.ts`))[key];
}
// 权限判定所需的工具能力表：逐项来自 TS 工具元数据，避免 Rust 侧硬编码再次漂移。
const capabilities = Object.fromEntries(
  builtInTools
    .map((entry) => [entry.metadata.name, entry])
    .map(([name, entry]) => {
      const m = entry.metadata;
      return [
        name,
        {
          allowedInPlanMode: m.allowedInPlanMode ?? false,
          readOnly: m.readOnly,
          destructive: m.destructive,
          requiresUserInteraction: m.requiresUserInteraction === true,
          sideEffectScope: m.sideEffectScope,
          riskLevel: m.riskLevel,
          needsApproval: m.needsApproval,
          // permission 声明在 entry 上（不在 metadata），permission 字段即规则匹配用的能力名。
          ...(entry.permission?.permission ? { permissionName: entry.permission.permission } : {}),
          ...(entry.permission?.alwaysAsk === true ? { alwaysAsk: true } : {}),
        },
      ];
    })
    .sort(([a], [b]) => (a < b ? -1 : 1)),
);

for (const [file, data] of [
  ["tool_capabilities.json", capabilities],
  [
    "agent_memory_templates.json",
    Object.fromEntries(
      // TS 在 rootDir 后追加宿主平台的 path.sep；生成资产若直接落 sep，会随生成机器变化，
      // 在 Windows 上 --check 必然漂移。改写成 {sep} 占位，由 Rust 运行时按本机分隔符填充。
      ["user", "project", "local"].map((scope) => [
        scope,
        buildPersistentAgentMemoryPrompt({
          rootDir: "{memoryRoot}",
          indexContent: "{memoryIndex}",
          scope,
        }).replaceAll(`{memoryRoot}${sep}`, "{memoryRoot}{sep}"),
      ]),
    ),
  ],
  [
    "../domain/agent_profiles.json",
    normalizeAgentProfiles([]).map((p) => ({
      ...p,
      systemPrompt:
        (p.name === "Explore" ? buildExploreAgentPrompt({}) : p.systemPrompt) +
        "\n\n" +
        buildSubagentCommonNotes(),
    })),
  ],
  [
    "agent_descriptions.json",
    {
      Agent: createAgentToolEntry({ dynamicWorkflowEnabled: false }).metadata.description,
      SendMessage: sendMessageToolEntry.metadata.description,
    },
  ],
  ["tool_schemas.json", schemas],
  ["skill_description.json", skillToolEntry.metadata.description],
  ["plugin_defaults.json", [...DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS].sort()],
  [
    "todo_descriptions.json",
    {
      TodoRead: todoReadToolEntry.metadata.description,
      TodoWrite: todoWriteToolEntry.metadata.description,
    },
  ],
  ["question_description.json", askUserQuestionToolEntry.metadata.description],
  [
    "plan_mode_descriptions.json",
    {
      EnterPlanMode: enterPlanModeToolEntry.metadata.description,
      ExitPlanMode: exitPlanModeToolEntry.metadata.description,
    },
  ],
  ["../domain/plan_mode_reminders.json", planModeReminders],
]) {
  const directory = file.startsWith("../domain/") ? "domain" : "tools";
  const name = file.replace("../domain/", "");
  const path = new URL(`../apps/zcode-cli-rust/crates/${directory}/src/${name}`, import.meta.url);
  const formatted = await format(path.pathname, `${JSON.stringify(data, null, 2)}\n`);
  if (formatted.errors.length) throw new Error(`Cannot format Rust tool asset: ${file}`);
  const content = formatted.code;
  if (process.argv.includes("--check")) {
    if ((await readFile(path, "utf8")) !== content)
      throw new Error(`Rust tool asset differs from TS: ${file}`);
  } else await writeFile(path, content);
}
