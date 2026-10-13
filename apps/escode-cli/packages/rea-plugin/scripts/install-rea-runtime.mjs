// 按目标平台把 REA 运行时安装到插件包的 node_modules/（CI 打包与手工 dev 构建共用）。
//
// 为什么装到插件根 node_modules/ 而不是 vendor/：seed walker（bootstrap/src/app/bundled-plugins.ts
// 的 shouldSkipDirectory）只在 depth === 0 且顶层白名单含该名时才放行 node_modules，
// vendor/node_modules（深度 1）会被静默丢弃，运行时进不了插件缓存。插件根 node_modules 与
// computer-use 的 runtimeTopLevelPaths: ["node_modules"] 完全同构。
//
// REA 的 dist/ 是 tsc 产物而非自包含 bundle（40 个运行时依赖，含 node-pty、evmole 等原生模块），
// 因此必须按目标平台执行一次真实安装 —— 这就是「打包进来」的代价：约 100 MB/平台。
// 版本不在本文件里写死：唯一事实源是插件包 package.json 的 escodePlugin.reaAgentsVersion。
//
// 目标平台默认是运行本脚本的机器（发布的 desktop 矩阵本来就是每个平台在自己的 runner 上构建）。
// 需要在一台机器上为别的平台备料时用 --os/--cpu/--libc 覆盖，npm 会据此挑选 optional/platform
// 依赖（实测：在 Windows 上装 darwin-arm64 只带 node-pty-darwin-arm64，不带 win32 变体）。
//
// 注意：本插件是 pnpm workspace 成员，`pnpm install` 可能把这份不属于 lockfile 的 node_modules
// 清掉（computer-use 的 sharp/koffi 同样是这种情况）。构建脚本因此必须可重复执行 —— 本脚本幂等，
// 重跑即可恢复。
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

function readFlag(name) {
  const index = process.argv.indexOf(name);
  return index === -1 ? undefined : process.argv[index + 1];
}

const pluginRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const pinnedVersion = JSON.parse(readFileSync(join(pluginRoot, "package.json"), "utf8"))
  ?.escodePlugin?.reaAgentsVersion;
if (typeof pinnedVersion !== "string" || pinnedVersion.trim() === "") {
  throw new Error("插件 package.json 缺少 escodePlugin.reaAgentsVersion");
}

const target = {
  cpu: readFlag("--cpu") ?? process.arch,
  libc: readFlag("--libc") ?? null,
  os: readFlag("--os") ?? process.platform,
  version: pinnedVersion.trim(),
};
const targetArgs = [
  ...(readFlag("--os") ? ["--os", target.os] : []),
  ...(readFlag("--cpu") ? ["--cpu", target.cpu] : []),
  ...(readFlag("--libc") ? ["--libc", target.libc] : []),
];

const entry = join(pluginRoot, "node_modules", "rea-agents", "scripts", "rea.mjs");
const markerPath = join(pluginRoot, "node_modules", ".rea-runtime.json");
const isSameTarget = (marker) =>
  marker?.version === target.version &&
  marker?.os === target.os &&
  marker?.cpu === target.cpu &&
  (marker?.libc ?? null) === target.libc;

// 幂等：打包与 dev 都会走到这一步，每次重复下载 100 MB 不可接受。
// 版本与目标平台都没变且入口就位时直接复用；换平台或换版本会因不匹配而重装。
if (existsSync(entry) && existsSync(markerPath)) {
  if (isSameTarget(JSON.parse(readFileSync(markerPath, "utf8")))) {
    process.stdout.write(
      `[rea] runtime already current: rea-agents@${target.version} (${target.os}-${target.cpu})\n`,
    );
    process.exit(0);
  }
}

const npmArgs = [
  "install",
  `rea-agents@${target.version}`,
  "--prefix",
  pluginRoot,
  "--omit=dev",
  "--no-audit",
  "--no-fund",
  "--no-save",
  "--no-package-lock",
  ...targetArgs,
];
// Windows 上 npm 是 .cmd shim，Node 既不能直接 spawn（ENOENT / EINVAL），用 shell:true 又会命中
// DEP0190（参数不转义）。与 REA 自己的注册方式一致：经 cmd.exe /d /s /c，并自行给含空格的参数加引号
// —— 仓库路径带空格时裸拼命令会被拆成多个参数。
const quoted = npmArgs.map((arg) =>
  /[\s"&|^<>]/.test(arg) ? `"${arg.replaceAll('"', '""')}"` : arg,
);
const install =
  process.platform === "win32"
    ? spawnSync(
        process.env.ComSpec ?? "cmd.exe",
        ["/d", "/s", "/c", ["npm", ...quoted].join(" ")],
        { encoding: "utf8", stdio: "inherit" },
      )
    : spawnSync("npm", npmArgs, { encoding: "utf8", stdio: "inherit" });
if (install.status !== 0) {
  throw new Error(`npm install rea-agents@${target.version} 失败（exit ${install.status}）`);
}

if (!existsSync(entry)) throw new Error(`安装后仍缺少 REA 入口：${entry}`);
const installedVersion = JSON.parse(
  readFileSync(join(pluginRoot, "node_modules", "rea-agents", "package.json"), "utf8"),
).version;
if (installedVersion !== target.version) {
  throw new Error(`REA 版本不符：期望 ${target.version}，实际 ${installedVersion}`);
}

writeFileSync(
  markerPath,
  `${JSON.stringify({ ...target, installedAt: new Date().toISOString() }, null, 2)}\n`,
);
process.stdout.write(`[rea] runtime ready: rea-agents@${installedVersion} (${target.os}-${target.cpu})\n`);
