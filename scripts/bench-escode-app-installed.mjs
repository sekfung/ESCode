// 安装包口径的 App bench 配置（docs/specs/rust-unified-host.md 阶段 3 验收补齐）。
//
// 同一个已安装包内切换 Host 与 runtime，避免把 dev 构建链（tsup/vite，实测约 165s）算进冷启动：
//   A：Node Host + Node CLI（ESCODE_RUST_HOST=0 + ESCODE_AGENT_SERVER_RUNTIME=node）
//   B：Node Host + Rust CLI（ESCODE_RUST_HOST=0 + 包内 resources/glm/escode-cli-rust）
//   C：Rust Host + 内嵌 runtime（ESCODE_RUST_HOST=1，Host 内 runtime-link 默认 embedded）
// 就绪使用主进程日志而不是进程名轮询：数据库终态 ready + 首屏 did-finish-load（见
// `ESCODE_BENCH_READY_LOG` 的受控日志）。Linux 若没有 DISPLAY 则经 xvfb-run 启动，与
// installed 演练同一路径。

import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

export const BENCH_READY_LOG_ENV = "ESCODE_BENCH_READY_LOG";
export const DATABASE_READY_PATTERN = /\[database-startup\] terminal \{[^}]*?status: 'ready'/;
export const RENDERER_READY_PATTERN = /\[bench-ready\] main window did-finish-load/;
export const BENCH_READY_LOGS = [DATABASE_READY_PATTERN, RENDERER_READY_PATTERN];

function requireExisting(path, label) {
  if (!existsSync(path)) throw new Error(`${label} 不存在：${path}`);
  return path;
}

/** 解析 `--installed-app/--installed-glm`，缺省回落到 CI 写入的 ESCODE_INSTALLED_*。 */
export function resolveInstalledBenchPaths({ app, glm, env = process.env }) {
  const rawApp = app?.trim() || env.ESCODE_INSTALLED_APP?.trim();
  if (!rawApp) {
    throw new Error("安装包模式需要 --installed-app <应用可执行文件> 或 ESCODE_INSTALLED_APP");
  }
  const appPath = requireExisting(resolve(rawApp), "安装包可执行文件");
  const rawGlm = glm?.trim() || env.ESCODE_INSTALLED_GLM?.trim();
  const glmPath = requireExisting(
    rawGlm ? resolve(rawGlm) : join(dirname(appPath), "resources", "glm"),
    "安装包 resources/glm",
  );
  const rust = requireExisting(
    join(glmPath, process.platform === "win32" ? "escode-cli-rust.exe" : "escode-cli-rust"),
    "包内 Rust runtime",
  );
  return { app: appPath, glm: glmPath, rust };
}

/** Linux 无图形会话时用 xvfb-run（CI 与本地 headless 同一路径）。 */
export function installedLaunchCommand(appPath, env = process.env) {
  if (process.platform === "linux" && !env.DISPLAY?.trim()) {
    return ["xvfb-run", ["-a", appPath, "--no-sandbox"]];
  }
  return [appPath, process.platform === "linux" ? ["--no-sandbox"] : []];
}

const COMMON_ENV = {
  [BENCH_READY_LOG_ENV]: "1",
  ELECTRON_ENABLE_LOGGING: "1",
};

export function installedBenchConfigs(paths) {
  const command = installedLaunchCommand(paths.app);
  const base = {
    command,
    appBinary: paths.app,
    readyLogs: BENCH_READY_LOGS,
    isolatedProfile: true,
  };
  return {
    A: {
      ...base,
      label: "Node Host + Node CLI（安装包）",
      env: {
        ...COMMON_ENV,
        ESCODE_RUST_HOST: "0",
        ESCODE_AGENT_SERVER_RUNTIME: "node",
        ESCODE_AGENT_SERVER_COMMAND: "",
        ESCODE_AGENT_SERVER_ARGS_JSON: "",
      },
    },
    B: {
      ...base,
      label: "Node Host + Rust CLI（安装包）",
      env: {
        ...COMMON_ENV,
        ESCODE_RUST_HOST: "0",
        ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust",
        ESCODE_AGENT_SERVER_COMMAND: paths.rust,
        ESCODE_AGENT_SERVER_ARGS_JSON: JSON.stringify(["app-server", "--stdio"]),
      },
    },
    C: {
      ...base,
      label: "Rust Host + 内嵌 runtime（安装包）",
      env: {
        ...COMMON_ENV,
        ESCODE_RUST_HOST: "1",
        ESCODE_HOST_RUNTIME_LINK: "embedded",
      },
    },
  };
}

/**
 * 安装包主进程不是 `electron` 可执行文件名（linux-unpacked 下是 escode-preview），
 * 进程树判定因此按可执行文件路径识别，而不是按进程名前缀。
 */
export function isInstalledAppProcess(entry, appBinary) {
  if (!appBinary) return false;
  return entry.commandLine.includes(appBinary);
}
