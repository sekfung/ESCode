// CI：在 runner 上安装 release 工作流产出的桌面安装包，并验证安装结果可用（docs/specs/rust-release-rollback.md
// 「安装包真机演练」）。
//   node scripts/ci-install-desktop.mjs <安装包目录>
// - Windows：NSIS 静默安装（/S /D=…）；macOS：挂载 dmg 拷出 .app；Linux：apt 安装 deb。
// - 启动检查：应用主进程起来后超时时间内不退出（Linux 用 xvfb-run），且数据库启动终态为 ready。
//   在空临时目录下启动：runner 的 cwd 是仓库 checkout，会掩盖/诱发打包态与开发入口探测的耦合。
// - 把应用可执行文件与 resources/glm 写进 $GITHUB_ENV（ESCODE_INSTALLED_APP / ESCODE_INSTALLED_GLM），供
//   escode-cli-rust-installed.test.ts 用包内两种 runtime 做 Node → Rust → Node 回退演练。
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";

const launchOnly = process.argv.includes("--launch-only");
const source = resolve(process.argv.slice(2).find((arg) => !arg.startsWith("--")) ?? "artifacts");
const temp = process.env.RUNNER_TEMP ?? resolve(".ci-install");
const pick = (suffix) => {
  const files = readdirSync(source);
  const found = files.find((name) => name.endsWith(suffix));
  if (!found) throw new Error(`No *${suffix} in ${source}: ${files.join(", ")}`);
  return join(source, found);
};
const run = (command, args, options = {}) => {
  const result = spawnSync(command, args, { stdio: "inherit", ...options });
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} failed: ${result.status ?? result.signal}`);
};
const capture = (command, args) => {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed: ${result.stderr}`);
  return result.stdout;
};

let app;
let glm;
if (launchOnly) {
  // 第二个启动阶段：复用上一阶段写入 $GITHUB_ENV 的安装结果，不重复安装。
  app = process.env.ESCODE_INSTALLED_APP?.trim();
  glm = process.env.ESCODE_INSTALLED_GLM?.trim();
  if (!app || !glm) {
    throw new Error(
      "--launch-only requires ESCODE_INSTALLED_APP and ESCODE_INSTALLED_GLM from a previous install",
    );
  }
} else if (process.platform === "win32") {
  const installer = pick(".exe");
  const target = join(temp, "escode-install");
  // NSIS 约定：/D= 必须是最后一个参数且不加引号（runner 临时目录不含空格）。
  run(installer, ["/S", `/D=${target}`]);
  const exe = readdirSync(target).find(
    (name) => name.endsWith(".exe") && !/^uninstall/i.test(name),
  );
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
    .find((line) => line.endsWith("/resources/glm/zcode.cjs"));
  if (!entry) throw new Error(`Installed package ${pkg} has no resources/glm/zcode.cjs`);
  glm = dirname(entry);
  const base = dirname(dirname(glm));
  app = join(base, pkg);
}
for (const path of [app, glm, join(glm, "zcode.cjs")]) {
  if (!existsSync(path)) throw new Error(`Installed path missing: ${path}`);
}
console.log(`installed app: ${app}\ninstalled glm: ${glm}`);

// 启动检查：主进程在超时时间内不崩，且数据库启动终态为 ready（无签名的 macOS 包在 CI 上无 Gatekeeper 隔离属性，可直接运行）。
// Bugfix：之前只看进程存活，数据库启动失败（如 unsupported_runtime）时应用仍停在错误页不退出，作业照样通过。
// 主进程把 "[database-startup] terminal { …, status: '<phase>', … }" 打到 stdout，这里据此判定终态。
// Intel macOS runner 冷启动到 dom-ready 已需 13s，Host 随后才 spawn；20s 会误判，留足启动预算。
const STARTUP_TIMEOUT_MS = 60_000;
const DATABASE_TERMINAL_PATTERN = /\[database-startup\] terminal \{[^}]*?status: '([a-z_]+)'/;
const RUST_HOST_STARTED_PATTERN = /\[rust-host\] started at \d+\.\d+\.\d+\.\d+:\d+ \(pid \d+\)/;
// 默认翻转前的安装包验收：包内 Rust Host 必须真实启动并挂上 renderer，且不能再出现
// utility Node 窗口 Host。ESCODE_INSTALLED_EXPECT_RUST_HOST=1 时启用这些断言。
const expectRustHost = process.env.ESCODE_INSTALLED_EXPECT_RUST_HOST === "1";
const assertRustHostLaunch = (output) => {
  if (!expectRustHost) return;
  if (!RUST_HOST_STARTED_PATTERN.test(output)) {
    throw new Error(
      "Installed app did not start the packaged Rust Host in ESCODE_RUST_HOST=1 mode",
    );
  }
  if (!output.includes("attached renderer to Rust Host")) {
    throw new Error("Installed renderer did not attach to the packaged Rust Host");
  }
  if (output.includes("forked host process for (local-")) {
    throw new Error("Installed app spawned a Node utility window Host in Rust mode");
  }
};
const launchCwd = mkdtempSync(join(temp, "escode-launch-"));
await new Promise((done, fail) => {
  const [command, args] =
    process.platform === "linux" ? ["xvfb-run", ["-a", app, "--no-sandbox"]] : [app, []];
  const child = spawn(command, args, {
    cwd: launchCwd,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, ELECTRON_ENABLE_LOGGING: "1" },
    // POSIX 下自成进程组，结束时整组回收（见 stop）。
    detached: process.platform !== "win32",
  });
  let output = "";
  let databaseStatus;
  let settled = false;
  const stop = () => {
    child.removeAllListeners("exit");
    if (process.platform === "win32") {
      spawnSync("taskkill", ["/pid", String(child.pid), "/T", "/F"]);
    } else {
      // Bugfix：Linux 上 child 是 xvfb-run 脚本，SIGKILL 它不会连带结束 Electron/Xvfb；
      // 孤儿进程继续持有 stdout/stderr 管道，事件循环无法退出，installed (linux-x64) 卡到作业超时。
      // 按进程组整组 SIGKILL，并销毁管道，保证启动检查结束后脚本必然退出。
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch {
        child.kill("SIGKILL");
      }
    }
    child.stdout.destroy();
    child.stderr.destroy();
  };
  const finish = (error) => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    if (error) fail(error);
    else done();
  };
  const onData = (stream) => (chunk) => {
    stream.write(chunk);
    // 即使 DB 已 ready 也要继续累积：Rust Host 的 started/attach 日志出现在终态之后。
    output += chunk.toString();
    if (databaseStatus) return;
    const match = DATABASE_TERMINAL_PATTERN.exec(output);
    if (!match) return;
    databaseStatus = match[1];
    if (databaseStatus !== "ready") {
      stop();
      finish(new Error(`Database startup did not become ready: status=${databaseStatus}`));
    }
  };
  child.stdout.on("data", onData(process.stdout));
  child.stderr.on("data", onData(process.stderr));
  child.once("error", finish);
  child.once("exit", (code, signal) => {
    finish(new Error(`Application exited during startup: ${signal ?? code}`));
  });
  const timer = setTimeout(() => {
    stop();
    if (databaseStatus !== "ready") {
      finish(
        new Error(
          `Database startup reached no terminal state within ${STARTUP_TIMEOUT_MS / 1000}s`,
        ),
      );
      return;
    }
    console.log(
      `application stayed up for ${STARTUP_TIMEOUT_MS / 1000}s with database startup ready`,
    );
    try {
      assertRustHostLaunch(output);
      if (expectRustHost) {
        console.log("packaged Rust Host started and attached the renderer");
      }
    } catch (error) {
      finish(error);
      return;
    }
    finish();
  }, STARTUP_TIMEOUT_MS);
});

if (process.env.GITHUB_ENV) {
  appendFileSync(
    process.env.GITHUB_ENV,
    `ESCODE_INSTALLED_APP=${app}\nESCODE_INSTALLED_GLM=${glm}\n`,
  );
}
