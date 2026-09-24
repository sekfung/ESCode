// Run with node --import tsx. 导出 TS PermissionService 的判定矩阵，作为 Rust 权限模式的差分 oracle
// （docs/specs/rust-permission-modes.md「验收 1」）。
// 轴值写进产物，Rust 测试按同一份轴枚举，不在两端各自硬编码顺序。
import { readFile, writeFile } from "node:fs/promises";
import { PermissionService } from "../apps/zcode-cli/packages/core/src/permission/service.ts";

const toolsWithCapability = ["Write", "mcp__srv__tool"];
const toolsWithDefaults = [
  "Read",
  "Glob",
  "Grep",
  "WebSearch",
  "WebFetch",
  "TodoRead",
  "TodoWrite",
  "Write",
  "Edit",
  "Bash",
  "EnterPlanMode",
  "ExitPlanMode",
  "mcp__srv__tool",
  "UnknownTool",
];
// [mode, planEnabled]；planEnabled=null 表示缺省（按 mode==="plan" 推导）。
const modes = [
  ["build", null],
  ["build", true],
  ["edit", null],
  ["edit", true],
  ["plan", null],
  ["plan", false],
  ["yolo", null],
  ["yolo", true],
  ["auto", null],
  ["auto", true],
];
const flags = [
  "readOnly",
  "destructive",
  "alwaysAsk",
  "requiresUserInteraction",
  "needsApproval",
  "allowedInPlanMode",
];
const scopes = ["workspace", "session", "none"];
const risks = ["low", "high", "critical"];
const permissionNames = [null, "edit", "mcp"];

function capabilities() {
  const out = [];
  for (let bits = 0; bits < 1 << flags.length; bits++) {
    for (const sideEffectScope of scopes) {
      for (const riskLevel of risks) {
        for (const permission of permissionNames) {
          const cap = { sideEffectScope, riskLevel };
          flags.forEach((flag, i) => (cap[flag] = Boolean(bits & (1 << i))));
          if (permission) cap.permission = { permission };
          out.push(cap);
        }
      }
    }
  }
  return out;
}

const service = new PermissionService();
const outcomes = [];
let decisions = "";
function record(toolName, [mode, planEnabled], capability) {
  const result = service.checkPermission(
    {
      toolName,
      input: {},
      riskLevel: "low",
      mode,
      ...(planEnabled === null ? {} : { planEnabled }),
    },
    capability,
  );
  const key = `${result.decision}:${result.ruleId}`;
  let index = outcomes.indexOf(key);
  if (index < 0) index = outcomes.push(key) - 1;
  // 单字符编码结果下标，矩阵上万条仍保持产物体积可审阅。
  decisions += String.fromCharCode(48 + index);
}

// 枚举顺序：先 defaults（capability 缺省），再 withCapability；每段 tool → mode → capability。
for (const tool of toolsWithDefaults) for (const m of modes) record(tool, m, undefined);
const caps = capabilities();
for (const tool of toolsWithCapability)
  for (const m of modes) for (const c of caps) record(tool, m, c);

const content = `${JSON.stringify({
  axes: { toolsWithDefaults, toolsWithCapability, modes, flags, scopes, risks, permissionNames },
  outcomes,
  decisions,
})}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/permission_matrix.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Permission matrix differs from TS; run node --import tsx scripts/generate-zcode-cli-rust-permission-matrix.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
