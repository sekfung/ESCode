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

// ── 规则分支：项目 / 会话规则、WebFetch 预批、官方 CUA 作用域 ──
const cuaRule = "zcode:permission-capability:official_cua";
const rulesets = [
  null,
  { version: 1, deny: [{ toolName: "Bash", ruleContent: "rm:*" }] },
  { version: 1, ask: [{ toolName: "Write" }] },
  { version: 1, allow: [{ toolName: "Edit", ruleContent: "src/*" }] },
  { version: 1, allow: [{ toolName: "WebFetch", ruleContent: "domain:example.com" }] },
  { version: 1, deny: [{ toolName: "Bash", ruleContent: "" }], allow: [{ toolName: "Bash" }] },
  {
    version: 1,
    allow: [{ toolName: cuaRule }],
    deny: [{ toolName: "AlwaysAskTool", ruleContent: "*danger*" }],
  },
  { version: 1, allow: [{ toolName: "Bash", ruleContent: "git *" }] },
];
const sessionRules = [null, { version: 1, allow: [{ toolName: "AlwaysAskTool" }] }];
const inputs = [
  ["Bash", { command: "rm -rf x" }],
  ["Bash", { command: "rm" }],
  ["Bash", { command: "rmdir x" }],
  ["Bash", { command: "rm\tx" }],
  ["Bash", { command: "git status" }],
  ["Bash", { command: "git\nstatus" }],
  ["Bash", "rm -rf"],
  ["Write", { file_path: "src/a.ts" }],
  ["Edit", { file_path: "src/a.ts" }],
  ["Write", { file_path: "lib/a.ts" }],
  ["WebFetch", { url: "https://Example.com./x" }],
  ["WebFetch", { url: "https://docs.python.org/3/" }],
  ["WebFetch", { url: "https://vercel.com/docs/x" }],
  ["WebFetch", { url: "https://vercel.com/docs%2fx" }],
  ["WebFetch", { url: "https://vercel.com/docs%252Ex" }],
  ["WebFetch", { url: "https://vercel.com/doc" }],
  ["WebFetch", { url: "notaurl" }],
  ["AlwaysAskTool", { command: "a danger b" }],
  ["AlwaysAskTool", { command: "safe" }],
  ["CuaTool", { action: "click" }],
];
const ruleModes = ["build", "plan", "yolo", "edit"];
let ruleDecisions = "";
for (const project of rulesets) {
  for (const session of sessionRules) {
    for (const [toolName, input] of inputs) {
      for (const mode of ruleModes) {
        for (const official of toolName === "CuaTool" ? [false, true] : [false]) {
          const capability =
            toolName === "AlwaysAskTool"
              ? { alwaysAsk: true }
              : toolName === "CuaTool"
                ? {
                    sideEffectScope: "workspace",
                    ...(official ? { permissionCapabilityGroup: "official_cua" } : {}),
                  }
                : undefined;
          const service = new PermissionService();
          if (session)
            service.grantSessionPermission([
              { type: "addRules", behavior: "allow", destination: "session", rules: session.allow },
            ]);
          const result = service.checkPermission(
            { toolName, input, riskLevel: "low", mode },
            capability,
            project,
          );
          const key = `${result.decision}:${result.ruleId}`;
          let index = outcomes.indexOf(key);
          if (index < 0) index = outcomes.push(key) - 1;
          ruleDecisions += String.fromCharCode(48 + index);
        }
      }
    }
  }
}

// WebFetch 预批清单不导出：从 TS 源码抽取，作为 Rust 内嵌资产（--check 防漂移）。
const webfetchSource = await readFile(
  new URL("../apps/zcode-cli/packages/core/src/tool/webfetch-preapproved.ts", import.meta.url),
  "utf8",
);
const hostsBlock = webfetchSource.match(/PREAPPROVED_HOSTS = new Set\(\[([\s\S]*?)\]\)/)[1];
const prefixBlock = webfetchSource.match(
  /PREAPPROVED_PATH_PREFIXES = new Map\(\[([\s\S]*?)\]\);/,
)[1];
const webfetch = `${JSON.stringify({
  hosts: [...hostsBlock.matchAll(/"([^"]+)"/g)].map((m) => m[1]),
  pathPrefixes: Object.fromEntries(
    [...prefixBlock.matchAll(/\["([^"]+)", \[([^\]]*)\]\]/g)].map((m) => [
      m[1],
      [...m[2].matchAll(/"([^"]+)"/g)].map((x) => x[1]),
    ]),
  ),
})}
`;

const content = `${JSON.stringify({
  axes: { toolsWithDefaults, toolsWithCapability, modes, flags, scopes, risks, permissionNames },
  outcomes,
  decisions,
  // 规则段枚举顺序：project → session → input → mode →（CuaTool 时）official=false/true。
  ruleAxes: { rulesets, sessionRules, inputs, ruleModes },
  ruleDecisions,
})}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/permission_matrix.json",
  import.meta.url,
);
const webfetchTarget = new URL(
  "../apps/zcode-cli-rust/crates/domain/src/webfetch_preapproved.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if (
    (await readFile(target, "utf8")) !== content ||
    (await readFile(webfetchTarget, "utf8")) !== webfetch
  ) {
    throw new Error(
      "Permission matrix differs from TS; run node --import tsx scripts/generate-zcode-cli-rust-permission-matrix.mjs",
    );
  }
} else {
  await writeFile(target, content);
  await writeFile(webfetchTarget, webfetch);
}
