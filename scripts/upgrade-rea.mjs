// REA 升级：把「改 pin → 重装运行时 → 重新 vendor 技能 → 重生成 TS/Rust 生成物」收成一条命令。
//
// 为什么需要它：上游发布节奏接近每日（GitHub releases 的 tag 形如 rea-agents-X.Y.Z，带 Highlights /
// Changes / BREAKING CHANGES 段），而升级要同步四处**版本耦合**的工件：
//   1. rea-plugin/package.json 的 escodePlugin.reaAgentsVersion（唯一事实源）
//   2. vendor 的运行时（按平台安装，脚本幂等、按版本标记判断是否重装）
//   3. bootstrap/src/app/rea-auto-approved-tools.ts（免审批清单，带版本号）
//   4. crates/tools/src/official_plugins.json（Rust 侧随生成物拿到同一份清单）
// 漏掉 3/4 会被测试拦下（清单版本 != pin），但一条命令能省一次红灯。
//
// 用法：
//   node scripts/upgrade-rea.mjs            # 升到 npm 上的最新版
//   node scripts/upgrade-rea.mjs 6.5.0      # 升到指定版本
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginRoot = join(repoRoot, "apps", "escode-cli", "packages", "rea-plugin");
const skillName = "reverse-engineer-anything";
const pluginSkillDir = join(pluginRoot, "skills", skillName);

function run(command, args, options = {}) {
  process.stdout.write(`\n$ ${command} ${args.join(" ")}\n`);
  execFileSync(command, args, { cwd: repoRoot, stdio: "inherit", ...options });
}

function readPin() {
  return JSON.parse(readFileSync(join(pluginRoot, "package.json"), "utf8")).escodePlugin
    .reaAgentsVersion;
}

function hashDirectory(directory) {
  if (!existsSync(directory)) return "";
  const hash = createHash("sha256");
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true }).sort((a, b) =>
      a.name.localeCompare(b.name),
    )) {
      const full = join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else hash.update(entry.name).update(readFileSync(full));
    }
  };
  walk(directory);
  return hash.digest("hex");
}

const requested = process.argv[2]?.trim() || "latest";
// Windows 上 npm 是 .cmd shim，Node 直接 spawn 会 ENOENT（与插件安装脚本同一坑）。
const npmViewArgs = ["view", `rea-agents@${requested}`, "version"];
const resolved = (
  process.platform === "win32"
    ? execFileSync(process.env.ComSpec ?? "cmd.exe", ["/d", "/s", "/c", `npm ${npmViewArgs.join(" ")}`], {
        encoding: "utf8",
      })
    : execFileSync("npm", npmViewArgs, { encoding: "utf8" })
).trim();
const current = readPin();
if (resolved === current) {
  process.stdout.write(
    `REA 已是 ${current}（cwd 的 pin 与 npm 一致）；仍会重跑生成步骤以保证工件同步。\n`,
  );
} else {
  process.stdout.write(`REA ${current} -> ${resolved}\n`);

  // 1. 唯一事实源：pin。
  const packagePath = join(pluginRoot, "package.json");
  const packageJson = JSON.parse(readFileSync(packagePath, "utf8"));
  packageJson.escodePlugin.reaAgentsVersion = resolved;
  writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`);

  // 2. 运行时（按平台安装；脚本按版本标记判断是否需要重装）。
  run(process.execPath, [join(pluginRoot, "scripts", "install-rea-runtime.mjs")]);

  // 3. 重新 vendor 上游技能（LF 原字节；skills/** 已在 .gitattributes 钉住 LF）。
  const upstreamSkill = join(pluginRoot, "node_modules", "rea-agents", "skills", skillName);
  if (!existsSync(upstreamSkill)) {
    throw new Error(`运行时里没有技能目录：${upstreamSkill}`);
  }
  const before = hashDirectory(pluginSkillDir);
  rmSync(pluginSkillDir, { force: true, recursive: true });
  mkdirSync(dirname(pluginSkillDir), { recursive: true });
  cpSync(upstreamSkill, pluginSkillDir, { recursive: true });
  const after = hashDirectory(pluginSkillDir);
  process.stdout.write(
    before === after
      ? "\n技能内容未变（vendored 副本与上游一致）\n"
      : "\n技能内容已更新：P4 落地后需要重跑 node scripts/licenses.mjs notices（登记的是逐字节哈希）\n",
  );
}

// 4. 生成物：免审批清单（TS）与 Rust 侧清单。
run(process.execPath, [join(repoRoot, "scripts", "generate-rea-auto-approved-tools.mjs")]);
run(process.execPath, ["--import", "tsx", join(repoRoot, "scripts", "generate-escode-cli-rust-official-plugins.mjs")]);

process.stdout.write(
  [
    "\n完成。接下来请人工完成（脚本不代替判断）：",
    "  1. 读该版本的 GitHub release notes，重点看 BREAKING CHANGES：",
    "     改过入口形态（scripts/rea.mjs）、MCP 模式 argv、技能目录或新增必需 env 时才需要动代码；",
    `  2. 跑测试：pnpm --filter @escode/bootstrap exec vitest run tests/rea-plugin.test.ts`,
    "     与 cd apps/escode-cli-rust && cargo test --locked -p escode-cli-tools；",
    "  3. 跑一次真实分析（免引擎目标）并回填 docs/specs/rea-plugin.md 的验收记录。",
  ].join("\n") + "\n",
);
