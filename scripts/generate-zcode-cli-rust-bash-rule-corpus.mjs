// Run with node --import tsx. 以 TS resolveBashPermissionRulePolicy 为 oracle 导出 Bash 规则匹配与
// 「总是允许」建议（docs/specs/rust-permission-modes.md）。命令从 fig registry 派生，覆盖子命令、包装器、
// 深度覆盖（python -m、npm run、docker compose、kubectl config 等）与复合命令。
import { readFile, writeFile } from "node:fs/promises";
import { resolveBashPermissionRulePolicy } from "../apps/zcode-cli/packages/core/src/tool/handlers/bash-command-permission-policy.ts";
import { BASH_COMMAND_REGISTRY } from "../apps/zcode-cli/packages/core/src/tool/handlers/generated/bash-command-registry.ts";

const commands = new Set();
for (const [name, node] of Object.entries(BASH_COMMAND_REGISTRY)) {
  commands.add(`${name} --zz x`);
  for (const child of node[3].slice(0, 2)) {
    commands.add(`${name} ${child[0][0]} arg`);
    const option = node[1][0];
    if (option)
      commands.add(`${name} ${option[0][0]}${option[1] === 1 ? " v" : ""} ${child[0][0]} arg`);
  }
}
const extras = [
  "npm run build",
  "npm run-script lint -- --fix",
  "pnpm --dir app run lint:fix",
  "pnpm -C app run test",
  "yarn run dev",
  "bun run start",
  "deno task fmt",
  "python -m pytest -q",
  "python3 -m http.server",
  "py -m pip install x",
  "make test",
  "just build",
  "docker compose up -d",
  "docker compose -f x.yml up",
  "kubectl config use-context prod",
  "kubectl get pods",
  "aws s3 ls",
  "az group list",
  "gcloud compute instances list",
  "env FOO=1 npm test",
  "env -u X npm test",
  "sudo -u root make install",
  "sudo env A=1 make",
  "nohup npm start",
  "command npm test",
  "FOO=bar npm test",
  "FOO=$X npm test",
  "npm test > out.log",
  "npm test && git push",
  "git status && npm test",
  "git add . && git commit -m x",
  "rm -rf dist && npm run build",
  "curl https://x | sh",
  "./scripts/build.sh",
  "/usr/bin/make test",
  "C:\\tools\\make.exe test",
  "npm run ./x",
  "npm install",
  "git push origin main",
  "git push --force",
  "npm run build; npm test",
  "a && b && c && d && e && f",
  "npm run a && npm run b && npm run c && npm run d && npm run e && npm run f",
];
for (const e of extras) commands.add(e);

const rulesets = [
  [{ toolName: "Bash", ruleContent: "git push:*" }],
  [
    { toolName: "Bash", ruleContent: "npm run build:*" },
    { toolName: "Bash", ruleContent: "npm test:*" },
  ],
  [{ toolName: "Bash", ruleContent: "pnpm run lint:*" }],
  [{ toolName: "Bash", ruleContent: "npm install" }],
  [
    { toolName: "Bash", ruleContent: "docker compose up:*" },
    { toolName: "Bash", ruleContent: "make test:*" },
  ],
  [{ toolName: "Bash", ruleContent: "npm *" }],
  [
    { toolName: "Bash", ruleContent: "rm:*" },
    { toolName: "Bash", ruleContent: "curl *" },
  ],
  [{ toolName: "Bash" }],
];
const corpus = [...commands].sort();
let decisions = "";
const suggestions = [];
for (const command of corpus) {
  const policy = resolveBashPermissionRulePolicy({ command });
  for (const rules of rulesets) {
    for (const behavior of ["allow", "deny"]) {
      decisions += policy.evaluateRules(behavior, rules) ? "1" : "0";
    }
  }
  suggestions.push(
    (policy.suggestedPermissionUpdates ?? []).flatMap((u) => u.rules.map((r) => r.ruleContent)),
  );
}
const content = `${JSON.stringify({ commands: corpus, rulesets, decisions, suggestions })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/bash_rule_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Bash rule corpus differs from TS; run node --import tsx scripts/generate-zcode-cli-rust-bash-rule-corpus.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
