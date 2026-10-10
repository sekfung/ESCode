// CI：在 runner 上安装 release 工作流产出的桌面安装包，并验证安装结果可用（docs/specs/rust-release-rollback.md
// 「安装包真机演练」）。
//   node scripts/ci-install-desktop.mjs <安装包目录>
// - Windows：NSIS 静默安装（/S /D=…）；macOS：挂载 dmg 拷出 .app；Linux：apt 安装 deb。
// - 启动检查：应用主进程起来后 20s 内不退出（Linux 用 xvfb-run）。
// - 把应用可执行文件与 resources/glm 写进 $GITHUB_ENV（ESCODE_INSTALLED_APP / ESCODE_INSTALLED_GLM），供
//   escode-cli-rust-installed.test.ts 用包内两种 runtime 做 Node → Rust → Node 回退演练。
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readdirSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

const source = resolve(process.argv[2] ?? "artifacts");
const temp = process.env.RUNNER_TEMP ?? resolve(".ci-install");
const files = readdirSync(source);
const pick = (suffix) => {
  const found = files.find((name) => name.endsWith(suffix));
  if (!found) throw new Error(`No *${suffix} in ${source}: ${files.join(", ")}`);
  return join(source, found);
};
const run = (command, args, options = {}) => {
  const result = spawnSync(command, args, { stdio: "inherit", ...options });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed: ${result.status ?? result.signal}`);
};
const capture = (command, args) => {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed: ${result.stderr}`);
  return result.stdout;
};

let app;
let glm;
if (process.platform === "win32") {
  const installer = pick(".exe");
  const target = join(temp, "escode-install");
  // NSIS 约定：/D= 必须是最后一个参数且不加引号（runner 临时目录不含空格）。
  run(installer, ["/S", `/D=${target}`]);
  const exe = readdirSync(target).find((name) => name.endsWith(".exe") && !/^uninstall/i.test(name));
  if (!exe) throw new Error(`No application executable in ${target}`);
  app = join(target, exe);
  glm = join(target, "resources", "glm");
} else if (process.platform === "darwin") {
  const dmg = pick(".dmg");
  const mount = join(temp, "escode-dmg");
  mkdirSync(mount, { recursive: true });
  run("hdiutil", ["attach", "-nobrowse", "-readonly", "-mountpoint", mount, dmg]);
  try {
    const bundle = readdirSync(mount).find((name) => name.endsWith(".app"));
    if (!bundle) throw new Error(`No .app in ${dmg}`);
    const target = join(temp, bundle);
    run("ditto", [join(mount, bundle), target]);
    const macos = join(target, "Contents", "MacOS");
    const name = basename(bundle, ".app");
    app = join(macos, existsSync(join(macos, name)) ? name : readdirSync(macos)[0]);
    glm = join(target, "Contents", "Resources", "glm");
  } finally {
    run("hdiutil", ["detach", mount]);
  }
} else {
  const deb = pick(".deb");
  run("sudo", ["apt-get", "install", "-y", deb]);
  const pkg = capture("dpkg-deb", ["-f", deb, "Package"]).trim();
  const entry = capture("dpkg", ["-L", pkg])
    .split("\n")
    .find((line) => line.endsWith("/resources/glm/escode.cjs"));
  if (!entry) throw new Error(`Installed package ${pkg} has no resources/glm/escode.cjs`);
  glm = dirname(entry);
  const base = dirname(dirname(glm));
  app = join(base, pkg);
}
for (const path of [app, glm, join(glm, "escode.cjs")]) {
  if (!existsSync(path)) throw new Error(`Installed path missing: ${path}`);
}
console.log(`installed app: ${app}\ninstalled glm: ${glm}`);

// 启动检查：主进程 20s 内不崩即视为可启动（无签名的 macOS 包在 CI 上无 Gatekeeper 隔离属性，可直接运行）。
await new Promise((done, fail) => {
  const [command, args] =
    process.platform === "linux" ? ["xvfb-run", ["-a", app, "--no-sandbox"]] : [app, []];
  const child = spawn(command, args, { stdio: "inherit", env: { ...process.env, ELECTRON_ENABLE_LOGGING: "1" } });
  let exited = false;
  child.once("error", fail);
  child.once("exit", (code, signal) => {
    exited = true;
    fail(new Error(`Application exited during startup: ${signal ?? code}`));
  });
  setTimeout(() => {
    if (exited) return;
    child.removeAllListeners("exit");
    if (process.platform === "win32") spawnSync("taskkill", ["/pid", String(child.pid), "/T", "/F"]);
    else child.kill("SIGKILL");
    console.log("application stayed up for 20s");
    done();
  }, 20_000);
});

if (process.env.GITHUB_ENV) {
  appendFileSync(process.env.GITHUB_ENV, `ESCODE_INSTALLED_APP=${app}\nESCODE_INSTALLED_GLM=${glm}\n`);
}
