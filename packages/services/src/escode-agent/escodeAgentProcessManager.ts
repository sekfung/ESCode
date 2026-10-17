import { localRuntimeNetworkCapture } from "#src/escode-agent/localRuntimeNetworkCapture.js";
import { resolveESCodeAgentSpawnCwd } from "#src/escode-agent/escodeAgentSpawnCwd.js";
import type { ESCodeAgentStorageStartupSnapshot } from "#src/escode-agent/escodeAgent.js";
/* eslint-disable max-lines -- escodeAgentProcessManager 集中维护 agent 子进程启动、复用、超时回收和 runtime identity，拆分会扩大进程生命周期状态同步面 */
import { spawn } from "node:child_process";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { homedir } from "node:os";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { Emitter } from "@escode/rpc";
import { GEN_UI_OUTPUT_DIRECTORY, GEN_UI_OUTPUT_ROOT_ENV } from "@escode/shared/node";
import { getAppConfigDir } from "../paths.js";
import {
  parseESCodeProcessDiagnostic,
  ESCODE_AGENT_LIFECYCLE_LOG_MARKER,
  ESCODE_PROCESS_DIAGNOSTIC_NAME_MAX_CHARS,
  ESCODE_PROCESS_DIAGNOSTIC_MESSAGE_MAX_CHARS,
  ESCODE_PROCESS_DIAGNOSTIC_STACK_MAX_CHARS,
} from "@escode/shared/process-diagnostic";
import {
  ESCODE_AGENT_RUNTIME,
  ESCODE_AGENT_PROVIDER,
  ESCODE_DESKTOP_PACKAGED_ENV,
  ESCODE_REMOTE_PACKAGED_RUNTIME_ENV,
  ESCODE_RUNTIME_ENV_KEY,
  resolveWorkspaceKey,
  resolveESCodeRuntimeEnv,
  sanitizeESCodeRuntimeEnv,
} from "@escode/shared";
import {
  findESCodeAgentRuntimeBinary,
  findESCodeAgentRuntimeNodeBundle,
  findESCodeAgentRustBinary,
} from "../runtime-tools/providerRuntimeResolver.js";
import { isEffectiveDevelopmentNodeEnv } from "#src/runtime-tools/nodeEnv.js";
import { createServiceLogger } from "#src/logger/serviceLogger.js";
import { ESCodeProtocolClient } from "./escodeProtocolClient.js";
import { ESCodeStdioTransport } from "./escodeStdioTransport.js";
import { readESCodeStdioTapDevState } from "./escodeStdioTapDevConfig.js";
import type { ESCodeAgentPresentationSurface } from "./escodeAgentPresentationSurface.js";
import { shouldSpawnInDetachedProcessGroup } from "../process/processTreeTerminator.js";
import type { RuntimeProcessLifecycleReporter } from "../process/runtimeProcessLifecycle.js";
import { buildAgentWorkspaceIdentityEnv } from "../runtime-tools/agentProxyEnv.js";

export interface ESCodeAgentCommand {
  /** 本地配套 CLI bundle 的存储专用 Worker 入口；远端/自定义命令不推断能力。 */
  storagePreparationEntry?: string;
  /** 原生 Agent 通过同一命令的 --prepare-storage 子进程完成存储握手。 */
  storagePreparationMode?: "process";
  /** 本次部署的 Agent 支持迁移前的启动通知；旧自定义命令保持原协议。 */
  supportsStorageStartup?: boolean;
  /** 由 resolver 标记的 Rust runtime 命令；启动失败时 manager 据此回退 Node（rust-packaging.md）。 */
  runtime?: "escode-cli-rust";
  command: string;
  args?: string[];
  cwd?: string;
  env?: Record<string, string>;
}

export interface ESCodeAgentCommandResolverContext {
  presentationSurface?: ESCodeAgentPresentationSurface;
  workspacePath: string;
  workspaceIdentity?: string;
  workspaceKey: string;
}

export type ESCodeAgentCommandResolver = (
  context: ESCodeAgentCommandResolverContext,
) => Promise<ESCodeAgentCommand | null> | ESCodeAgentCommand | null;

export interface ESCodeAgentProcessManagerOptions {
  commandResolver?: ESCodeAgentCommandResolver;
  presentationSurface?: ESCodeAgentPresentationSurface;
  requestTimeoutMs?: number;
  processLifecycleReporter?: RuntimeProcessLifecycleReporter;
  /**
   * 进程泳道标识。同一 workspace 的不同泳道各走独立 manager 实例；lane 会写入
   * runtimeIdentity 与 spawn/exit 日志，便于排障区分。
   * 缺省为 chat 主泳道，不追加任何标记。
   */
  lane?: string;
  /**
   * 空闲回收阈值：连接上没有请求在飞持续超过该时长，就主动回收整棵进程树，
   * 下次 getClient 透明重新拉起。只给 mcp-status 这类“按需探测、进程内挂着 MCP 子进程”
   * 的控制面 lane 使用；chat / plugin 缺省不回收。
   */
  idleTimeoutMs?: number;
  /**
   * 仅当默认进程 cwd 等于目标 workspace 且该目录不可用时使用。
   * 业务 workspacePath/workspaceKey 不随 cwd 兜底改变。
   */
  spawnFallbackCwd?: string;
  /**
   * 每次 spawn agent 子进程前解析的额外环境变量（在 process.env 之后、workspace 变量之前合入）。
   * 用于把设置页的代理等配置注入子进程；按 spawn 时读取，天然「下次启动生效」。
   *
   * context 携带本次 spawn 的 workspace 标识三元组（workspacePath/workspaceIdentity/workspaceKey）。
   * Helper lifecycle 不再回收或重启已有 Agent。
   */
  resolveSpawnEnv?: (context: {
    workspacePath: string;
    workspaceIdentity?: string;
    workspaceKey: string;
  }) => Promise<Record<string, string>> | Record<string, string>;
  /**
   * 可选的外部 spawn admission hook。CUA 默认装配不再注入 Helper recovery gate，
   * 避免 Helper lifecycle 阻塞或间接重启 Agent；保留该通用 hook 供其他产品策略使用。
   */
  waitForSpawnAdmission?: (context: {
    workspacePath: string;
    workspaceIdentity?: string;
    workspaceKey: string;
    signal?: AbortSignal;
  }) => Promise<void> | void;
}

interface ManagedESCodeAgentProcess {
  client: ESCodeProtocolClient;
  child: ChildProcessWithoutNullStreams;
  cleanupPromise?: Promise<void>;
  exited: boolean;
  /**
   * 退出码/信号。就绪前退出的归因只认这里的事实（75 = workspace owner 冲突），
   * 不能从 transport 关闭的文案反推；exit 事件晚于 stdout EOF 时由归因方等待本字段。
   */
  exitInfo?: { code: number | null; signal: NodeJS.Signals | null };
  /** 子进程退出（或 spawn 失败且无 pid）时 resolve；新一代 runtime 据此等待旧 owner 释放 workspace。 */
  exitPromise: Promise<void>;
  resolveExit: () => void;
  /** 首次进入回收编排（含重试）后的整体结束信号，不论成败；用于给“等待旧 owner”提供生命周期边界。 */
  retirementSettled?: Promise<void>;
  firstCleanupReason?: AgentProcessCleanupReason;
  idleTimer?: ReturnType<typeof setTimeout>;
  readyAt?: number;
  readyReported: boolean;
  runtimeIdentity: ESCodeAgentRuntimeIdentity;
  runtimeInstanceId: string;
  spawned: boolean;
  startedAt: number;
  terminationIntent?: AgentProcessTerminationIntent;
  workspace: {
    workspacePath: string;
    workspaceIdentity?: string;
  };
}

type AgentProcessCleanupReason =
  | "idle-timeout"
  | "idle-timeout-retry"
  | "manager-dispose"
  | "manager-dispose-retry"
  | "protocol-close"
  | "request-timeout"
  | "workspace-dispose"
  | "workspace-dispose-retry";

interface AgentProcessTerminationIntent {
  kind: "expected" | "watchdog_recycle";
  reason: Exclude<AgentProcessCleanupReason, "protocol-close">;
  requestedAt: number;
}

const E2E_COVERAGE_PRELOAD_SOURCE = `
const { takeCoverage } = require("node:v8");
const { writeFileSync } = require("node:fs");
const { resolve } = require("node:path");
const coverageDirectory = process.env.NODE_V8_COVERAGE;
let coverageFlushStarted = false;
for (const signal of process.platform === "win32"
  ? ["SIGINT", "SIGTERM"]
  : ["SIGINT", "SIGTERM", "SIGHUP"]) {
  const flushCoverage = () => {
    if (coverageFlushStarted) return;
    coverageFlushStarted = true;
    if (coverageDirectory) {
      writeFileSync(
        resolve(coverageDirectory, \`coverage-signal-\${process.pid}.marker\`),
        signal,
      );
    }
    try {
      takeCoverage();
    } catch (error) {
      if (coverageDirectory) {
        writeFileSync(
          resolve(coverageDirectory, \`coverage-signal-error-\${process.pid}.txt\`),
          error instanceof Error ? error.stack || error.message : String(error),
        );
      }
    }
    if (process.listenerCount(signal) !== 1) return;
    setTimeout(() => {
      process.exit(signal === "SIGINT" ? 130 : signal === "SIGTERM" ? 143 : 129);
    }, 1000);
  };
  process.prependListener(signal, flushCoverage);
}
if (coverageDirectory) {
  writeFileSync(resolve(coverageDirectory, \`coverage-ready-\${process.pid}.marker\`), "");
}
`;

export function buildE2EAgentCoverageEnv(
  env: NodeJS.ProcessEnv = process.env,
): Record<string, string> {
  const artifactDir = env.ESCODE_E2E_ARTIFACT_DIR?.trim();
  if (env.ESCODE_E2E_COVERAGE !== "1" || !artifactDir) {
    return {};
  }
  const directory = resolve(artifactDir, "coverage", "raw", "cli");
  mkdirSync(directory, { recursive: true });
  const preloadPath = resolve(directory, "escode-e2e-coverage-preload.cjs");
  // CLI bundle 未压缩时解析耗时可能超过 E2E 的早退窗口，普通 shutdown
  // handler 尚未注册就收到 SIGTERM。用 NODE_OPTIONS preload 在解析 bundle 前接管落盘。
  writeFileSync(preloadPath, E2E_COVERAGE_PRELOAD_SOURCE, "utf8");
  const requireOption = `--require=${JSON.stringify(preloadPath)}`;
  return {
    NODE_OPTIONS: [env.NODE_OPTIONS?.trim(), requireOption].filter(Boolean).join(" "),
    NODE_V8_COVERAGE: directory,
  };
}

/**
 * （CLI 重连重订边界）：同一 workspace 的 agent 进程被重建（超时回收/崩溃后
 * 首个 getClient 重新拉起）。v4 订阅（sessions-index/workspace-config/conversation）
 * 都活在 CLI 进程内存里，进程换代即失效——订阅方收到本事件后必须重发 subscribe。
 */
export interface ESCodeAgentRuntimeRestartedEvent {
  workspaceKey: string;
  runtimeIdentity: ESCodeAgentRuntimeIdentity;
}

export interface ESCodeAgentRuntimeLifecycleEvent {
  workspacePath: string;
  workspaceIdentity?: string;
  workspaceKey: string;
  runtimeIdentity: ESCodeAgentRuntimeIdentity;
  state: "available" | "unavailable";
}

export interface ESCodeAgentRuntimeIdentity {
  generation: number;
  identity: string;
  processId?: number;
  workspaceKey: string;
  /** 进程泳道标识；chat 主泳道缺省为空。 */
  lane?: string;
}

interface ESCodeAgentSpawnPreflight {
  command: string;
  args: string[];
  requestedCwd: string;
  cwd: string;
  cwdSource: "command" | "workspace" | "workspace-fallback";
  commandPathKind: "absolute" | "path-search";
  commandExists: boolean | null;
  cwdExists: boolean;
}

const serviceLog = createServiceLogger("escode-agent");

const log = (...args: unknown[]) => serviceLog.info(undefined, ...args);
const warnLog = (...args: unknown[]) => serviceLog.warn(undefined, ...args);
const errorLog = (...args: unknown[]) => serviceLog.error(undefined, ...args);

const AGENT_STDERR_TAIL_MAX_LINES = 20;
const AGENT_STDERR_LINE_MAX_CHARS = 1_000;
const AGENT_STDERR_SENSITIVE_ASSIGNMENT_PATTERN =
  /(["']?(?:api[-_]?key|authorization|cookie|credential|password|secret|token)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;}]+)/gi;
const AGENT_STDERR_AUTH_SCHEME_PATTERN = /\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+/gi;
const AGENT_STDERR_API_KEY_PATTERN = /\b(sk-)[A-Za-z0-9_-]{16,}\b/gi;

function redactAgentDiagnostic(value: string): string {
  return (
    value
      .replace(AGENT_STDERR_SENSITIVE_ASSIGNMENT_PATTERN, "$1<redacted>")
      .replace(AGENT_STDERR_AUTH_SCHEME_PATTERN, "$1 <redacted>")
      // 裸 key 也必须在跨进程诊断和生产日志之前遮盖。
      .replace(AGENT_STDERR_API_KEY_PATTERN, "$1<redacted>")
  );
}

const debugLog = (...args: unknown[]) => {
  if (!isEffectiveDevelopmentNodeEnv()) {
    return;
  }
  serviceLog.debug(undefined, ...args);
};

function createAgentStderrTail(): {
  append(line: string): void;
  snapshot(): { lineCount: number; tail: string[] };
} {
  const tail: string[] = [];
  let lineCount = 0;

  return {
    append(line) {
      lineCount += 1;
      const redacted = redactAgentDiagnostic(line);
      const bounded =
        redacted.length > AGENT_STDERR_LINE_MAX_CHARS
          ? `${redacted.slice(0, AGENT_STDERR_LINE_MAX_CHARS)}…[truncated]`
          : redacted;
      tail.push(bounded);
      if (tail.length > AGENT_STDERR_TAIL_MAX_LINES) {
        tail.shift();
      }
    },
    snapshot() {
      return { lineCount, tail: [...tail] };
    },
  };
}

function parseArgsJson(raw: string | undefined): string[] | undefined {
  const trimmed = raw?.trim();
  if (!trimmed) {
    return undefined;
  }
  const parsed = JSON.parse(trimmed) as unknown;
  if (!Array.isArray(parsed) || parsed.some((item) => typeof item !== "string")) {
    throw new Error("ESCODE_AGENT_SERVER_ARGS_JSON must be a JSON string array");
  }
  return parsed;
}

function findUpward(relativePath: string): string | null {
  let current = process.cwd();
  while (true) {
    const candidate = join(current, relativePath);
    if (existsSync(candidate)) {
      return candidate;
    }
    const parent = dirname(current);
    if (parent === current) {
      return null;
    }
    current = parent;
  }
}

async function buildESCodeAgentSpawnPreflight(
  command: ESCodeAgentCommand,
  workspacePath: string,
  spawnFallbackCwd?: string,
): Promise<ESCodeAgentSpawnPreflight> {
  const commandPathKind = isAbsolute(command.command) ? "absolute" : "path-search";
  const requestedCwd = command.cwd ?? workspacePath;
  const {
    cwd,
    usedFallback: shouldUseWorkspaceFallback,
    cwdExists,
  } = await resolveESCodeAgentSpawnCwd({ requestedCwd, workspacePath, spawnFallbackCwd });
  return {
    command: command.command,
    args: command.args ?? [],
    requestedCwd,
    cwd,
    cwdSource: shouldUseWorkspaceFallback
      ? "workspace-fallback"
      : command.cwd === undefined
        ? "workspace"
        : "command",
    commandPathKind,
    // Node spawn 的 ENOENT 既可能来自 command 缺失，也可能来自 cwd 缺失。
    // 生产日志在 spawn 前同时记录两者可见性，避免把工作区路径丢失误判成自动更新丢 binary。
    commandExists: commandPathKind === "absolute" ? existsSync(command.command) : null,
    cwdExists,
  };
}

function resolveBundledWorkspaceESCodeAgentCommand(
  context: ESCodeAgentCommandResolverContext,
): ESCodeAgentCommand | null {
  const distEntrypoint = findUpward("apps/escode-cli/packages/cli/dist/escode.cjs");
  if (distEntrypoint) {
    const useBytecode =
      process.versions.electron && process.env.ESCODE_DESKTOP_AGENT_BYTECODE === "1";
    const entrypoint = useBytecode
      ? join(dirname(distEntrypoint), "escode.bytecode.cjs")
      : distEntrypoint;
    // 此同步 command resolver 沿用既有 existsSync 契约；显式试验不能静默回退成 JS。
    if (useBytecode && !existsSync(entrypoint)) {
      throw new Error("桌面 Agent 字节码入口缺失，请运行 pnpm build:desktop-agent:bytecode");
    }
    return {
      command: process.execPath,
      args: [entrypoint, "app-server", "--stdio"],
      // Worker 与 Electron Node 子进程的 V8 snapshot 可不同；临时存储准备继续用 JS。
      storagePreparationEntry: distEntrypoint,
      cwd: context.workspacePath,
      // 桌面端 host 运行在 Electron utility process 中，process.execPath 指向 Electron Helper。
      // 这里显式启用 Node 运行模式，避免内置 escode-agent 被当成 Electron/Chromium 子进程启动并卡在 GPU 初始化。
      env: { ELECTRON_RUN_AS_NODE: "1" },
    };
  }

  const sourceEntrypoint = findUpward("apps/escode-cli/packages/cli/src/main.ts");
  const tsxEntrypoint = findUpward("node_modules/.bin/tsx");
  if (!sourceEntrypoint || !tsxEntrypoint) {
    return null;
  }
  return {
    command: tsxEntrypoint,
    args: [sourceEntrypoint, "app-server", "--stdio"],
    cwd: context.workspacePath,
  };
}

/**
 * 打包运行时：桌面安装包的 Local Host，或桌面安装包部署并拉起的远程 server。二者都随包带 Rust 二进制，
 * 默认 runtime 为 Rust 且不回退 Node（docs/specs/rust-packaging.md「不回退 Node」「远程环境使用 Rust」）。
 */
function isPackagedAgentRuntime(): boolean {
  return (
    process.env[ESCODE_DESKTOP_PACKAGED_ENV] === "1" ||
    process.env[ESCODE_REMOTE_PACKAGED_RUNTIME_ENV] === "1"
  );
}

/**
 * monorepo 开发入口（dist / tsx 源码）。
 * Bugfix：findUpward 从 process.cwd() 向上探测，安装包在仓库目录下启动时（CI installed job、开发者从终端启动）
 * 会误命中仓库源码入口：默认 runtime 退回 Node，且 tsx 入口没有 storagePreparationEntry，数据库启动报
 * unsupported_runtime。打包态的 runtime 选择不能取决于 cwd，因此 Main 注入打包标记时整体跳过开发入口探测。
 */
function resolveWorkspaceDevESCodeAgentCommand(
  context: ESCodeAgentCommandResolverContext,
): ESCodeAgentCommand | null {
  if (isPackagedAgentRuntime()) return null;
  return resolveBundledWorkspaceESCodeAgentCommand(context);
}

function resolveDeployedESCodeAgentBinaryCommand(
  context: ESCodeAgentCommandResolverContext,
): ESCodeAgentCommand | null {
  // 旧 resolver 只识别 ESCODE_AGENT_SERVER_COMMAND env 和 monorepo 源码树。
  // SSH 远端把 zcode-server.cjs 单文件部署到 ~/.escode/server/，宿主进程的 cwd 不在仓库内、
  // env 也不会被 ssh exec 继承，即使 escode-agent 已经部署到 ~/.escode/server/agents/glm/，
  // resolver 也找不到，第一次 getClient 就抛 "ESCode agent server command is not configured"。
  // 这里复用 findESCodeAgentRuntimeBinary 的候选链（含 GLM_BINARY_PATH env、
  // packagedResourcesPath、~/.escode/server/agents/glm、bundled-agents 等），
  // 把已部署的原生 binary 当成最终兜底，远端/桌面打包形态都能命中。
  const binaryPath = findESCodeAgentRuntimeBinary();
  if (!binaryPath) {
    return null;
  }
  return {
    command: binaryPath,
    args: ESCODE_AGENT_RUNTIME.spawnArgs,
    cwd: context.workspacePath,
  };
}

function resolveElectronRuntimeESCodeAgentCommand(
  context: ESCodeAgentCommandResolverContext,
): ESCodeAgentCommand | null {
  // 桌面打包态：host 跑在 Electron utility process 里，process.execPath 指向 Electron Helper，
  // 它内置的 Node runtime 与 escode-cli 目标版本一致（Electron 41 = Node 24.x）。
  // 这里直接用 app 自带的 Electron Node 执行打进 resources/glm 的 zcode.cjs，
  // 不再随包内置一份独立 Node 二进制（体积从 ~180MB 降到 ~16MB，且跨平台同一份 JS）。
  // 用 process.versions.electron 作为闸门：远端 SSH/WSL host 由系统 Node 运行、没有 electron，
  // 会跳过这里继续走原生二进制兜底，桌面/远端两条链路互不影响。
  if (!process.versions.electron) {
    return null;
  }
  const bundlePath = findESCodeAgentRuntimeNodeBundle();
  if (!bundlePath) {
    return null;
  }
  return {
    command: process.execPath,
    args: [bundlePath, ...ESCODE_AGENT_RUNTIME.spawnArgs],
    storagePreparationEntry: bundlePath,
    cwd: context.workspacePath,
    // 关键：必须以纯 Node 模式启动，否则子进程会被当成 Electron/Chromium 子进程卡在 GPU 初始化。
    env: { ELECTRON_RUN_AS_NODE: "1" },
  };
}

/** Rust runtime 的启动参数：Host 注入原始 workspacePath（OS cwd 会 realpath 化，丢失本地 identity fallback）。 */
function rustRuntimeArgs(args: string[], workspacePath: string): string[] {
  if (args.some((arg) => arg === "--cwd" || arg.startsWith("--cwd=")))
    throw new Error(
      "escode-cli-rust --cwd is supplied by the Host; remove it from ESCODE_AGENT_SERVER_ARGS_JSON",
    );
  return [...args, "--cwd", workspacePath];
}

/**
 * Rust runtime 自行 seed 官方插件时，插件 MCP 服务仍由 Node 插件宿主运行（docs/specs/rust-official-plugin-seed.md）：
 * 传入与 Node runtime 相同的 Electron-as-Node 可执行文件与 escode.cjs 入口，Rust 据此改写 plugin.json，
 * 与 Node seed 的结果逐字节一致。开发态 tsx 源码入口不传（TS 在该形态同样不产出可复用的前缀）。
 */
function rustPluginHostEnv(
  context: ESCodeAgentCommandResolverContext,
): Pick<ESCodeAgentCommand, "env"> {
  const node =
    resolveWorkspaceDevESCodeAgentCommand(context) ??
    resolveElectronRuntimeESCodeAgentCommand(context);
  const entrypoint = node?.args?.[0];
  if (node && entrypoint?.endsWith(".cjs")) {
    return {
      env: {
        ESCODE_PLUGIN_HOST_EXEC_PATH: node.command,
        ESCODE_PLUGIN_HOST_ENTRYPOINT: entrypoint,
      },
    };
  }
  return remotePluginHostEnv();
}

/**
 * 远端没有 Electron：插件宿主用远程部署的独立 node（与 zcode-server.cjs 同一份）执行 agents/glm/zcode.cjs，
 * 与远端 wrapper（escodeAgentBundleWrapper）同一布局。
 */
function remotePluginHostEnv(): Pick<ESCodeAgentCommand, "env"> {
  if (process.env[ESCODE_REMOTE_PACKAGED_RUNTIME_ENV] !== "1") return {};
  const runtimeRoot =
    process.env.ESCODE_SERVER_RUNTIME_ROOT?.trim() || join(homedir(), ".escode", "server");
  const nodePath = join(runtimeRoot, "node");
  const entrypoint = findESCodeAgentRuntimeNodeBundle();
  if (!entrypoint || !existsSync(nodePath)) return {};
  return {
    env: {
      ESCODE_PLUGIN_HOST_EXEC_PATH: nodePath,
      ESCODE_PLUGIN_HOST_ENTRYPOINT: entrypoint,
    },
  };
}

export interface ESCodeAgentCommandResolverDeps {
  findRustBinary(): string | null;
  /** monorepo 开发态的 Node 入口（源码 / dist）；在场时默认 runtime 保持 Node。缺省按仓库结构探测。 */
  findDevCommand?(context: ESCodeAgentCommandResolverContext): ESCodeAgentCommand | null;
}

export function resolveDefaultESCodeAgentCommand(
  context: ESCodeAgentCommandResolverContext,
  deps: ESCodeAgentCommandResolverDeps = { findRustBinary: () => findESCodeAgentRustBinary() },
): ESCodeAgentCommand | null {
  // 运行时选择的唯一入口（docs/specs/rust-packaging.md）：默认用随包 Rust runtime，找不到二进制直接报错、
  // 就绪前失败也不改选 Node（不自动回退）；ESCODE_AGENT_SERVER_RUNTIME=node 是显式选择 Node。monorepo 开发态
  // （源码 / dist 入口在场）与自定义命令保持原有 Node 语义：开发改源码要立刻生效，自定义命令的协议形态由调用方决定。
  const runtime = process.env.ESCODE_AGENT_SERVER_RUNTIME?.trim() || undefined;
  if (runtime && runtime !== "escode-cli-rust" && runtime !== "node") {
    throw new Error("Unsupported ESCODE_AGENT_SERVER_RUNTIME");
  }
  // 默认 Rust 属于打包运行时（桌面安装包，或其部署的远程 server，二者都随包带 Rust 二进制）。开发态 App
  // 部署的远端只有 JS bundle，未注入标记时保持 Node 链。
  const defaultRust =
    runtime === undefined &&
    isPackagedAgentRuntime() &&
    !process.env.ESCODE_AGENT_SERVER_COMMAND?.trim() &&
    (deps.findDevCommand ?? resolveWorkspaceDevESCodeAgentCommand)(context) === null;
  const rust = runtime === "escode-cli-rust" || defaultRust;
  const baseArgs = parseArgsJson(process.env.ESCODE_AGENT_SERVER_ARGS_JSON) ?? [
    "app-server",
    "--stdio",
  ];
  const rustCommand = (command: string, cwd: string): ESCodeAgentCommand => ({
    runtime: "escode-cli-rust",
    command,
    storagePreparationMode: "process",
    supportsStorageStartup: true,
    args: rustRuntimeArgs(baseArgs, context.workspacePath),
    cwd,
    ...rustPluginHostEnv(context),
  });
  const command = process.env.ESCODE_AGENT_SERVER_COMMAND?.trim();
  if (command) {
    const cwd = process.env.ESCODE_AGENT_SERVER_CWD?.trim() || context.workspacePath;
    return applyPresentationSurfaceToCommand(
      rust ? rustCommand(command, cwd) : { command, args: baseArgs, cwd },
      context.presentationSurface,
    );
  }
  if (rust) {
    const binary = deps.findRustBinary();
    if (binary) {
      return applyPresentationSurfaceToCommand(
        rustCommand(binary, context.workspacePath),
        context.presentationSurface,
      );
    }
    // 修复原因：旧实现在这里静默改用 Node，Rust 期会话（rust-sessions.sqlite）随之全部打不开。
    // 选中 Rust 却没有随包二进制属于安装包缺陷，直接报错，不切换 runtime。
    errorLog("escode-cli-rust runtime selected but no bundled binary was found", {
      event: "escode_agent.runtime.rust_binary_missing",
      explicit: runtime === "escode-cli-rust",
    });
    throw new Error(
      "ESCode Rust runtime (escode-cli-rust) is not installed. Reinstall ESCode, or set ESCODE_AGENT_SERVER_RUNTIME=node to use the Node runtime explicitly.",
    );
  }

  // 顺序：env 显式覆盖 → monorepo dev 源码/dist（dev 改源码立刻生效，不会被远端历史装的 native binary
  // 抢先匹配）→ 桌面打包态 Electron Node runtime 跑 zcode.cjs → 已部署 native binary（远端 SSH 兜底）。
  const bundled =
    resolveWorkspaceDevESCodeAgentCommand(context) ??
    resolveElectronRuntimeESCodeAgentCommand(context);
  return applyPresentationSurfaceToCommand(
    bundled
      ? { ...bundled, supportsStorageStartup: true }
      : resolveDeployedESCodeAgentBinaryCommand(context),
    context.presentationSurface,
  );
}

function applyPresentationSurfaceToCommand(
  command: ESCodeAgentCommand | null,
  presentationSurface: ESCodeAgentCommandResolverContext["presentationSurface"],
): ESCodeAgentCommand | null {
  if (!command || presentationSurface !== "desktop") {
    return command;
  }

  const commandArgs = command.args ?? [];
  const args: string[] = [];
  for (let index = 0; index < commandArgs.length; index += 1) {
    const arg = commandArgs[index]!;
    if (arg === "--surface") {
      const nextArg = commandArgs[index + 1];
      // Bug 原因：旧逻辑无条件消费下一个 token，孤立的 --surface 会把后续
      // --stdio 等 option 一并吞掉，导致自定义 Agent 命令失去协议启动参数。
      // 只有明确的非 option value 才属于 --surface；其他 option 继续走原参数链路。
      if (nextArg !== undefined && !nextArg.startsWith("-")) {
        index += 1;
      }
      continue;
    }
    if (arg.startsWith("--surface=")) {
      continue;
    }
    args.push(arg);
  }

  return {
    ...command,
    args: [...args, "--surface", "desktop"],
  };
}

export function wrapESCodeAgentCommandWithStdioTapDevProxy(
  command: ESCodeAgentCommand,
  workspaceKey: string,
): ESCodeAgentCommand {
  const tapState = readESCodeStdioTapDevState();
  if (!tapState.enabled) {
    return command;
  }

  const tapScript = findUpward("scripts/dev/escode-stdio-tap.mjs");
  if (!tapScript) {
    debugLog("ESCode stdio tap proxy enabled but script not found");
    return command;
  }

  // 开发态 raw stdio 帧数据量和消息流同级，不能打进普通 info 日志。
  // 这里只在显式开关打开时用旁路 proxy 写盘，生产构建和默认开发路径都不受影响。
  return {
    supportsStorageStartup: command.supportsStorageStartup,
    command: process.execPath,
    args: [
      tapScript,
      "--workspace-key",
      workspaceKey,
      "--log-dir",
      tapState.logDir,
      "--",
      command.command,
      ...(command.args ?? []),
    ],
    cwd: command.cwd,
    env: {
      ...command.env,
      ELECTRON_RUN_AS_NODE: "1",
    },
  };
}

/**
 * Rust runtime 因 workspace owner 锁被其他进程持有而退出的退出码（apps/escode-cli-rust
 * WORKSPACE_OWNED_EXIT_CODE）。这是所有权冲突，不是 runtime 故障，不能触发 Node 回退。
 */
export const RUST_WORKSPACE_OWNED_EXIT_CODE = 75;

/**
 * owner 冲突退避窗口：锁的持有者是另一个存活实例，重试必须等它退出；无节流的 spawn 重试会形成风暴
 * （2026-10-13 现场 130–175 次/分钟）。首个窗口足够短，让「持有者刚好退出」的常规场景快速恢复。
 */
const WORKSPACE_OWNED_BACKOFF_BASE_MS = 1_000;
const WORKSPACE_OWNED_BACKOFF_MAX_MS = 5_000;
/** stdout EOF 可能先于 exit 事件；归因就绪前退出码时等待 exit 的上界。 */
const STARTUP_EXIT_CODE_GRACE_MS = 250;

/**
 * workspace owner 锁被另一个存活实例持有：就绪前以退出码 75 退出。等持有者退出后重试即可，
 * 因此不再按请求节奏反复 spawn，而是如实上报（docs/specs/rust-packaging.md「owner 冲突如实上报与启动退避」）。
 */
export class ESCodeAgentWorkspaceOwnedError extends Error {
  constructor() {
    super("The workspace runtime is already owned by another instance; close it and retry.");
    this.name = "ESCodeAgentWorkspaceOwnedError";
  }
}

export class ESCodeAgentProcessManager {
  private readonly processesByWorkspaceKey = new Map<string, ManagedESCodeAgentProcess>();
  private readonly ownedProcesses = new Set<ManagedESCodeAgentProcess>();
  private readonly startingByWorkspaceKey = new Map<string, Promise<ESCodeProtocolClient>>();
  private readonly restartGenerationByWorkspaceKey = new Map<string, number>();
  private readonly runtimeGenerationByWorkspaceKey = new Map<string, number>();
  private readonly availableRuntimeIdentityByWorkspaceKey = new Map<string, string>();
  /** owner 冲突退避：持有者是另一存活实例，窗口内不再 spawn。 */
  private readonly workspaceOwnedConflictByWorkspaceKey = new Map<
    string,
    { attempts: number; retryAt: number }
  >();
  private readonly startAdmissionAbortControllersByWorkspaceKey = new Map<
    string,
    Set<AbortController>
  >();
  private readonly storageStartupEmitter = new Emitter<{
    workspaceKey: string;
    snapshot: ESCodeAgentStorageStartupSnapshot;
  }>();
  readonly onStorageStartupChanged = this.storageStartupEmitter.event;

  getStorageStartupState(params: {
    workspacePath: string;
    workspaceIdentity?: string;
  }): ESCodeAgentStorageStartupSnapshot | null {
    const managed = this.processesByWorkspaceKey.get(resolveWorkspaceKey(params));
    return managed
      ? {
          generation: managed.runtimeIdentity.generation,
          state: managed.client.storageStartup.snapshot ?? null,
        }
      : null;
  }

  private readonly commandResolver: ESCodeAgentCommandResolver;
  private readonly presentationSurface: ESCodeAgentProcessManagerOptions["presentationSurface"];
  private readonly requestTimeoutMs: number | undefined;
  private readonly processLifecycleReporter: RuntimeProcessLifecycleReporter | undefined;
  private readonly resolveSpawnEnv: ESCodeAgentProcessManagerOptions["resolveSpawnEnv"];
  private readonly waitForSpawnAdmission: ESCodeAgentProcessManagerOptions["waitForSpawnAdmission"];
  private readonly spawnFallbackCwd: string | undefined;
  private readonly lane: string | undefined;
  private readonly idleTimeoutMs: number | undefined;
  private readonly runtimeRestartedEmitter = new Emitter<ESCodeAgentRuntimeRestartedEvent>();
  private readonly runtimeLifecycleEmitter = new Emitter<ESCodeAgentRuntimeLifecycleEvent>();
  private disposeAllInFlight: Promise<void> | undefined;
  private disposed = false;

  /** 进程换代通知（generation>1 时触发）；v4 订阅方据此重订，见 ESCodeAgentRuntimeRestartedEvent。 */
  readonly onRuntimeRestarted = this.runtimeRestartedEmitter.event;
  /** 进程真实 spawn 后 available，当前 protocol client 关闭后 unavailable。 */
  readonly onRuntimeLifecycle = this.runtimeLifecycleEmitter.event;

  constructor(options?: ESCodeAgentProcessManagerOptions) {
    this.commandResolver = options?.commandResolver ?? resolveDefaultESCodeAgentCommand;
    this.presentationSurface = options?.presentationSurface;
    this.requestTimeoutMs = options?.requestTimeoutMs;
    this.processLifecycleReporter = options?.processLifecycleReporter;
    this.resolveSpawnEnv = options?.resolveSpawnEnv;
    this.waitForSpawnAdmission = options?.waitForSpawnAdmission;
    this.spawnFallbackCwd = options?.spawnFallbackCwd;
    this.lane = options?.lane?.trim() || undefined;
    this.idleTimeoutMs =
      options?.idleTimeoutMs && options.idleTimeoutMs > 0 ? options.idleTimeoutMs : undefined;
  }

  private reportProcessLifecycle(
    callback: (reporter: RuntimeProcessLifecycleReporter) => void,
  ): void {
    const reporter = this.processLifecycleReporter;
    if (!reporter) {
      return;
    }

    try {
      callback(reporter);
    } catch (error) {
      // 进程生命周期上报是旁路观测，临时失败不得阻断 agent 启动或回收。
      warnLog("ESCode agent process lifecycle reporter failed", error);
    }
  }

  private clearIdleTimer(managed: ManagedESCodeAgentProcess): void {
    if (managed.idleTimer) {
      clearTimeout(managed.idleTimer);
      delete managed.idleTimer;
    }
  }

  /**
   * 空闲回收：每次在飞请求归零就重置计时；到点时若仍无请求在飞且该进程仍是当前活跃实例，
   * 主动回收整棵进程树（含挂在其下的 MCP 子进程）。归因为 expected/idle-timeout，
   * 不会被监控当作崩溃。到点时有新请求在飞则什么都不做，等下一次归零重新计时。
   */
  private scheduleIdleReclaim(workspaceKey: string, managed: ManagedESCodeAgentProcess): void {
    if (!this.idleTimeoutMs || this.disposed || managed.exited) {
      return;
    }
    this.clearIdleTimer(managed);
    const timer = setTimeout(() => {
      delete managed.idleTimer;
      if (this.disposed || managed.exited) {
        return;
      }
      if (this.processesByWorkspaceKey.get(workspaceKey) !== managed) {
        return;
      }
      // 自定义 Agent 的旧启动请求可能先结清，再继续数据库准备；无在飞 RPC 不代表迁移空闲。
      if (
        managed.client.pendingOperationRequestCount > 0 ||
        managed.client.storageStartup.isWaiting
      ) {
        return;
      }
      log("ESCode agent process idle timeout; reclaiming", {
        workspaceKey,
        pid: managed.child.pid,
        runtimeIdentity: managed.runtimeIdentity.identity,
        idleTimeoutMs: this.idleTimeoutMs,
      });
      this.processesByWorkspaceKey.delete(workspaceKey);
      this.reportRuntimeUnavailable(managed);
      void this.cleanupManagedProcessWithRetry(
        managed,
        "idle-timeout",
        "idle-timeout-retry",
        "idle timeout",
      ).catch(() => undefined);
    }, this.idleTimeoutMs);
    // 空闲计时器不能把 host 进程钉在事件循环里。
    timer.unref?.();
    managed.idleTimer = timer;
  }

  private recordTerminationIntent(
    managed: ManagedESCodeAgentProcess,
    reason: AgentProcessCleanupReason,
  ): void {
    if (managed.firstCleanupReason) {
      return;
    }
    managed.firstCleanupReason = reason;
    if (reason === "protocol-close") {
      return;
    }
    // protocol close 可能是 Agent 崩溃的结果，只有 Host 主动发起的回收
    // 才能建立退出意图。首次 cleanup 原因是根因事实，后续 app quit 等幂等回收不能
    // 把已经发生的异常 protocol close 改写成 expected。
    managed.terminationIntent = {
      kind: reason === "request-timeout" ? "watchdog_recycle" : "expected",
      reason,
      requestedAt: Date.now(),
    };
  }

  private reportRuntimeUnavailable(managed: ManagedESCodeAgentProcess): void {
    const workspaceKey = managed.runtimeIdentity.workspaceKey;
    if (
      this.availableRuntimeIdentityByWorkspaceKey.get(workspaceKey) !==
      managed.runtimeIdentity.identity
    ) {
      return;
    }
    this.availableRuntimeIdentityByWorkspaceKey.delete(workspaceKey);
    this.runtimeLifecycleEmitter.fire({
      workspacePath: managed.workspace.workspacePath,
      ...(managed.workspace.workspaceIdentity
        ? { workspaceIdentity: managed.workspace.workspaceIdentity }
        : {}),
      workspaceKey,
      runtimeIdentity: managed.runtimeIdentity,
      state: "unavailable",
    });
  }

  /**
   * Rust 进程就绪前失败只做诊断记录，不改变后续 runtime 选择（docs/specs/rust-packaging.md「不回退 Node」）。
   * 修复原因：旧实现在这里置位只增不减的 rustRuntimeFailed，之后本 manager 一律改用 Node；Rust 期会话
   * 存在 rust-sessions.sqlite，Node 读不到，打开历史会话即报 fault.subscribe.sessionNotFound。
   */
  private noteRustStartupFailure(
    command: ESCodeAgentCommand,
    managed: ManagedESCodeAgentProcess,
    reason: "spawn_error" | "exit_before_ready",
    exitCode?: number | null,
  ): void {
    if (command.runtime !== "escode-cli-rust" || managed.readyAt != null) {
      return;
    }
    if (exitCode === RUST_WORKSPACE_OWNED_EXIT_CODE) {
      warnLog("ESCode agent Rust runtime found the workspace owned by another runtime", {
        event: "escode_agent.runtime.rust_workspace_owned",
        workspaceKey: managed.runtimeIdentity.workspaceKey,
        runtimeIdentity: managed.runtimeIdentity.identity,
      });
      return;
    }
    errorLog("ESCode agent Rust runtime failed before ready", {
      event: "escode_agent.runtime.rust_startup_failed",
      reason,
      ...(exitCode != null ? { exitCode } : {}),
      workspaceKey: managed.runtimeIdentity.workspaceKey,
      runtimeIdentity: managed.runtimeIdentity.identity,
    });
  }

  private reportRuntimeReady(managed: ManagedESCodeAgentProcess): void {
    if (
      !managed.spawned ||
      managed.exited ||
      managed.readyReported ||
      managed.readyAt == null ||
      typeof managed.child.pid !== "number"
    ) {
      return;
    }
    managed.readyReported = true;
    this.reportProcessLifecycle((reporter) =>
      reporter.onReady?.({
        pid: managed.child.pid!,
        provider: ESCODE_AGENT_PROVIDER,
        ...(this.lane ? { lane: this.lane } : {}),
        workspacePath: managed.workspace.workspacePath,
        readyAt: managed.readyAt!,
        startupDurationMs: Math.max(0, managed.readyAt! - managed.startedAt),
        runtimeGeneration: managed.runtimeIdentity.generation,
        runtimeInstanceId: managed.runtimeInstanceId,
      }),
    );
  }

  private cleanupManagedProcess(
    managed: ManagedESCodeAgentProcess,
    reason: AgentProcessCleanupReason,
    options: { reportError?: boolean } = {},
  ): Promise<void> {
    this.recordTerminationIntent(managed, reason);
    this.clearIdleTimer(managed);
    if (managed.cleanupPromise) {
      return managed.cleanupPromise;
    }

    // protocol close 只会使 client 不再可复用，不代表它对应的
    // OS 进程已退出。将回收 Promise 绑在 managed process 上，timeout、restart 和
    // app quit 可以共用同一次幂等回收，Host 也不会丢失已退休进程的所有权。
    let cleanupCompleted = false;
    const cleanupPromise = managed.client
      .disposeAndWait()
      .then(() => {
        cleanupCompleted = true;
        log("ESCode agent process cleanup completed", {
          workspaceKey: managed.runtimeIdentity.workspaceKey,
          pid: managed.child.pid,
          runtimeIdentity: managed.runtimeIdentity.identity,
          reason,
        });
      })
      .finally(() => {
        if (managed.cleanupPromise === cleanupPromise) {
          delete managed.cleanupPromise;
        }
        if (cleanupCompleted) {
          this.ownedProcesses.delete(managed);
        }
      });
    managed.cleanupPromise = cleanupPromise;
    // Bug 原因：dispose 与重试编排竞态时，rejection 可能先于调用方 await 挂靠被判为
    // unhandledRejection（vitest 计为运行级失败，生产等价于未处理拒绝）。catch 必须在
    // 创建点无条件挂靠：reportError:false（重试路径）时静默吞掉，统一上报由
    // cleanupManagedProcessWithRetry 的 await/catch 完成，告警语义不变。
    void cleanupPromise.catch((error) => {
      if (options.reportError === false) {
        return;
      }
      errorLog("ESCode agent process cleanup failed", {
        workspaceKey: managed.runtimeIdentity.workspaceKey,
        pid: managed.child.pid,
        runtimeIdentity: managed.runtimeIdentity.identity,
        reason,
        error,
      });
    });
    return cleanupPromise;
  }

  private async cleanupManagedProcessForShutdown(
    managed: ManagedESCodeAgentProcess,
  ): Promise<void> {
    await this.cleanupManagedProcessWithRetry(
      managed,
      "manager-dispose",
      "manager-dispose-retry",
      "manager dispose",
    );
  }

  private cleanupManagedProcessWithRetry(
    managed: ManagedESCodeAgentProcess,
    reason: AgentProcessCleanupReason,
    retryReason: AgentProcessCleanupReason,
    retryScope: string,
  ): Promise<void> {
    const run = this.runCleanupManagedProcessWithRetry(managed, reason, retryReason, retryScope);
    managed.retirementSettled ??= run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  private async runCleanupManagedProcessWithRetry(
    managed: ManagedESCodeAgentProcess,
    reason: AgentProcessCleanupReason,
    retryReason: AgentProcessCleanupReason,
    retryScope: string,
  ): Promise<void> {
    try {
      // 首次 cleanup 的进程树/exit 观察可能只是中间态；在 retry 成功时，
      // 首次 rejection 不应提前升级为生产 error 告警。最终失败仍由本方法统一上报一次。
      await this.cleanupManagedProcess(managed, reason, { reportError: false });
    } catch (firstError) {
      // Windows 进程表查询/exit 事件可能短暂落后，首次 cleanup 会误报 root
      // 残留。restart/app quit 都不能把这种中间态暴露给调用方，需重试一次并复用
      // transport 内部快照；真实残留会在第二次 cleanup 继续抛出。
      const cleanupError = firstError as NodeJS.ErrnoException;
      warnLog(`ESCode agent process cleanup retrying during ${retryScope}`, {
        workspaceKey: managed.runtimeIdentity.workspaceKey,
        pid: managed.child.pid,
        runtimeIdentity: managed.runtimeIdentity.identity,
        cleanupStage: reason,
        errorName: firstError instanceof Error ? firstError.name || "Error" : "UnknownError",
        ...(typeof cleanupError.code === "string" ? { errorCode: cleanupError.code } : {}),
        error: firstError,
      });
      try {
        await this.cleanupManagedProcess(managed, retryReason, { reportError: false });
      } catch (finalError) {
        errorLog("ESCode agent process cleanup failed", {
          workspaceKey: managed.runtimeIdentity.workspaceKey,
          pid: managed.child.pid,
          runtimeIdentity: managed.runtimeIdentity.identity,
          reason: retryReason,
          retryScope,
          error: finalError,
        });
        throw finalError;
      }
    }
  }

  /**
   * 返回可用的 protocol client。命令声明 `supportsStorageStartup` 时，返回即代表存储已就绪：
   * 就绪前进程退出、启动超时、owner 冲突都在这里以归因后的错误抛出（owner 冲突见
   * `ESCodeAgentWorkspaceOwnedError`），不会把一个注定失败的 client 交给调用方。
   */
  async getClient(params: {
    workspacePath: string;
    workspaceIdentity?: string;
  }): Promise<ESCodeProtocolClient> {
    if (this.disposed) {
      throw new Error("ESCode agent process manager is disposed.");
    }
    const workspaceKey = resolveWorkspaceKey(params);
    const existing = this.processesByWorkspaceKey.get(workspaceKey);
    if (existing && !existing.child.killed) {
      return existing.client;
    }

    const starting = this.startingByWorkspaceKey.get(workspaceKey);
    if (starting) {
      const waitStartedAt = Date.now();
      log("ESCode agent process start already in progress", {
        workspaceKey,
      });
      const client = await starting;
      log("ESCode agent process start wait completed", {
        workspaceKey,
        durationMs: Date.now() - waitStartedAt,
      });
      return client;
    }

    // agent 启动前置后，host warmup 和 UI 首次 readWorkspacePresentation/sendPrompt
    // 可能同时进入 getClient。这里按 workspaceKey 收敛启动中的 promise，避免同一工作区重复 spawn。
    const startGeneration = this.restartGenerationByWorkspaceKey.get(workspaceKey) ?? 0;
    const admissionAbortController = new AbortController();
    let controllers = this.startAdmissionAbortControllersByWorkspaceKey.get(workspaceKey);
    if (!controllers) {
      controllers = new Set<AbortController>();
      this.startAdmissionAbortControllersByWorkspaceKey.set(workspaceKey, controllers);
    }
    controllers.add(admissionAbortController);
    const startPromise = this.startClient(
      params,
      workspaceKey,
      startGeneration,
      admissionAbortController.signal,
    );
    this.startingByWorkspaceKey.set(workspaceKey, startPromise);
    try {
      return await startPromise;
    } finally {
      controllers.delete(admissionAbortController);
      if (controllers.size === 0) {
        this.startAdmissionAbortControllersByWorkspaceKey.delete(workspaceKey);
      }
      if (this.startingByWorkspaceKey.get(workspaceKey) === startPromise) {
        this.startingByWorkspaceKey.delete(workspaceKey);
      }
    }
  }

  /**
   * 只读取已经登记的 runtime client；被动 observer 使用本入口避免 getClient 的隐式 spawn。
   */
  getExistingClient(params: {
    workspacePath: string;
    workspaceIdentity?: string;
  }): ESCodeProtocolClient | undefined {
    const managed = this.processesByWorkspaceKey.get(resolveWorkspaceKey(params));
    return managed && !managed.child.killed ? managed.client : undefined;
  }

  /** 资源管理器：当前仍存活的受管 runtime（pid + workspace + client） */
  listManagedProcesses(): Array<{
    pid: number;
    workspacePath: string;
    workspaceIdentity?: string;
    lane?: string;
    client: ESCodeProtocolClient;
  }> {
    const result: Array<{
      pid: number;
      workspacePath: string;
      workspaceIdentity?: string;
      lane?: string;
      client: ESCodeProtocolClient;
    }> = [];
    for (const managed of this.processesByWorkspaceKey.values()) {
      if (managed.exited || managed.child.killed || typeof managed.child.pid !== "number") continue;
      result.push({
        pid: managed.child.pid,
        workspacePath: managed.workspace.workspacePath,
        ...(managed.workspace.workspaceIdentity
          ? { workspaceIdentity: managed.workspace.workspaceIdentity }
          : {}),
        ...(this.lane ? { lane: this.lane } : {}),
        client: managed.client,
      });
    }
    return result;
  }

  /** Agent service 首次通过 provider/model 门禁后调用；同一 runtime 只上报一次。 */
  markReady(
    params: { workspacePath: string; workspaceIdentity?: string },
    client: ESCodeProtocolClient,
  ): void {
    const managed = this.processesByWorkspaceKey.get(resolveWorkspaceKey(params));
    if (!managed || managed.client !== client || managed.readyAt != null || managed.exited) {
      return;
    }
    managed.readyAt = Date.now();
    // getClient 可能早于 ChildProcess 的异步 spawn 事件返回，也可能在 await 期间
    // 被新 runtime 替换。只给返回该 entry 的进程标 ready，并由 spawn 回调保证 start → ready 顺序。
    this.reportRuntimeReady(managed);
  }

  /**
   * owner 冲突退避：每个窗口按连续冲突次数指数增长（1s → 2s → 4s，上限 5s）。窗口只限制 spawn 频率，
   * 不改变「持有者退出后即可恢复」的语义——成功接管（storage ready）时清除记录。
   */
  private noteWorkspaceOwnedConflict(workspaceKey: string): void {
    const conflict = this.workspaceOwnedConflictByWorkspaceKey.get(workspaceKey);
    const attempts = (conflict?.attempts ?? 0) + 1;
    const backoffMs = Math.min(
      WORKSPACE_OWNED_BACKOFF_BASE_MS * 2 ** (attempts - 1),
      WORKSPACE_OWNED_BACKOFF_MAX_MS,
    );
    this.workspaceOwnedConflictByWorkspaceKey.set(workspaceKey, {
      attempts,
      retryAt: Date.now() + backoffMs,
    });
  }

  private clearWorkspaceOwnedConflict(workspaceKey: string): void {
    this.workspaceOwnedConflictByWorkspaceKey.delete(workspaceKey);
  }

  /** 窗口内不 spawn：持有者仍是另一个存活实例，重复 spawn 只会再以 75 退出一次。 */
  private throwIfWorkspaceOwnedConflictActive(workspaceKey: string): void {
    const conflict = this.workspaceOwnedConflictByWorkspaceKey.get(workspaceKey);
    if (conflict && Date.now() < conflict.retryAt) {
      throw new ESCodeAgentWorkspaceOwnedError();
    }
  }

  /**
   * 存储启动是 client 的真实可用边界。就绪前把 client 交给调用方，失败只能被 transport 关闭统一翻译成
   * `SQLite startup failed: transport_closed`——owner 冲突、启动超时都会退化成与数据库无关的假象描述。
   * 这里等首个终态并在同一处归因，让调用方拿到的错误与真实原因一致。
   */
  private async waitForStorageStartupOrThrow(managed: ManagedESCodeAgentProcess): Promise<void> {
    const { client } = managed;
    if (!client.storageStartup.isWaiting) {
      return;
    }
    try {
      await client.storageStartup.wait();
      this.clearWorkspaceOwnedConflict(managed.runtimeIdentity.workspaceKey);
    } catch (error) {
      throw await this.classifyStartupFailure(managed, error);
    }
  }

  /**
   * 就绪前退出的归因：退出码是唯一事实（75 = owner 锁被另一进程持有）。stdout EOF 可能先于 exit 事件到达，
   * 因此在宽限窗口内等待 exit 取码；非 75 的失败维持原错误，不改写。
   */
  private async classifyStartupFailure(
    managed: ManagedESCodeAgentProcess,
    fallback: unknown,
  ): Promise<unknown> {
    if (managed.exitInfo === undefined && !managed.exited) {
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, STARTUP_EXIT_CODE_GRACE_MS);
        timer.unref?.();
        void managed.exitPromise.then(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
    return managed.exitInfo?.code === RUST_WORKSPACE_OWNED_EXIT_CODE
      ? new ESCodeAgentWorkspaceOwnedError()
      : fallback;
  }

  /**
   * 修复原因：请求超时 / 协议关闭会先把旧一代 runtime 移出注册表、再异步回收进程树。旧实现紧接着就为同一
   * workspace 拉起新一代，旧进程仍持有 Rust workspace owner 锁 → 新进程以 75 退出 → 被误判为 Rust 故障。
   * 这里在解析命令前等待同 workspace 的退休进程退出；边界由旧进程自身的回收编排给出（回收结束即放行，
   * 即使回收失败也不无限等待），不额外引入超时。
   */
  private async waitForRetiringRuntimes(workspaceKey: string): Promise<void> {
    const retiring = [...this.ownedProcesses].filter(
      (managed) =>
        managed.runtimeIdentity.workspaceKey === workspaceKey &&
        !managed.exited &&
        managed.retirementSettled !== undefined &&
        this.processesByWorkspaceKey.get(workspaceKey) !== managed,
    );
    if (retiring.length === 0) return;
    const waitStartedAt = Date.now();
    log("ESCode agent waiting for retiring runtime before start", {
      workspaceKey,
      pids: retiring.map((managed) => managed.child.pid),
    });
    await Promise.all(
      retiring.map((managed) =>
        Promise.race([managed.exitPromise, managed.retirementSettled as Promise<void>]),
      ),
    );
    log("ESCode agent retiring runtime wait completed", {
      workspaceKey,
      durationMs: Date.now() - waitStartedAt,
    });
  }

  private async startClient(
    params: {
      workspacePath: string;
      workspaceIdentity?: string;
    },
    workspaceKey: string,
    startGeneration: number,
    admissionSignal: AbortSignal,
  ): Promise<ESCodeProtocolClient> {
    const startStartedAt = Date.now();
    this.throwIfWorkspaceOwnedConflictActive(workspaceKey);
    await this.waitForRetiringRuntimes(workspaceKey);
    const resolveCommandStartedAt = Date.now();
    const command = await this.commandResolver({
      ...params,
      ...(this.presentationSurface ? { presentationSurface: this.presentationSurface } : {}),
      workspaceKey,
    });
    const resolveCommandDurationMs = Date.now() - resolveCommandStartedAt;
    if (!command) {
      throw new Error(
        "ESCode agent server command is not configured. Set ESCODE_AGENT_SERVER_COMMAND before integration.",
      );
    }
    if (admissionSignal.aborted) {
      throw admissionSignal.reason ?? new Error("ESCode agent process start was cancelled.");
    }
    const effectiveCommand = wrapESCodeAgentCommandWithStdioTapDevProxy(command, workspaceKey);
    log("ESCode agent command resolved", {
      workspaceKey,
      command: command.command,
      effectiveCommand: effectiveCommand.command,
      resolveCommandDurationMs,
    });

    // Helper recovery 可能在 command resolve 期间开始；先等待一次，确保 env 解析使用
    // recovery 后的 broker 凭据，而不是把旧状态带到 spawn 边界。
    await this.waitForSpawnAdmission?.({ ...params, workspaceKey, signal: admissionSignal });

    // 设置页代理等运行时 env 在 process.env 之后合入（覆盖继承的同名 shell 变量），
    // 但仍让 command.env（部署特定）保持最高优先级。
    const spawnEnv = (await this.resolveSpawnEnv?.({ ...params, workspaceKey })) ?? {};
    if (this.disposed) {
      // app 正在关闭时，启动中的 warmup 可能刚完成 command/env resolve。
      // 这时继续 spawn 会绕过 disposeAllAndWait 的快照，重新制造一个无人托管的 agent 进程。
      throw new Error("ESCode agent process manager is disposed.");
    }
    if ((this.restartGenerationByWorkspaceKey.get(workspaceKey) ?? 0) !== startGeneration) {
      // 切模型会重启单个 workspace。旧启动请求如果在重启后才恢复，
      // 不能继续 spawn 并写回进程池，否则新配置会被旧 agent 覆盖。
      throw new Error("ESCode agent process start was cancelled.");
    }
    // cwd 探测也让出事件循环，必须放在最终 admission 与销毁/代际检查之前。
    const spawnPreflight = await buildESCodeAgentSpawnPreflight(
      effectiveCommand,
      params.workspacePath,
      this.spawnFallbackCwd,
    );
    admissionSignal.throwIfAborted();
    // env resolve 本身是异步的，恢复屏障可能在这段时间重新关闭；必须在 spawn 前
    // 再等待并复查代际，不能只依赖第一次 admission。
    await this.waitForSpawnAdmission?.({ ...params, workspaceKey, signal: admissionSignal });
    if (this.disposed) {
      throw new Error("ESCode agent process manager is disposed.");
    }
    if ((this.restartGenerationByWorkspaceKey.get(workspaceKey) ?? 0) !== startGeneration) {
      throw new Error("ESCode agent process start was cancelled.");
    }
    // app 以本地开发方式启动时，让 agent 子进程也带上 ESCODE_RUNTIME_ENV=development；
    // 不再传 NODE_ENV，避免用户 shell/runtime 变量影响 ESCode 运行模式或泄漏到 Bash 工具。
    const runtimeEnv = resolveESCodeRuntimeEnv(process.env);
    log("ESCode agent spawn preflight", {
      workspaceKey,
      spawnPreflight,
    });
    const spawnRequestedAt = Date.now();
    const child = spawn(effectiveCommand.command, spawnPreflight.args, {
      cwd: spawnPreflight.cwd,
      // agent 可能再派生实际 runtime/MCP 子进程。POSIX 下让 wrapper 进入独立进程组，
      // 关闭时才能按进程树整体回收；Windows 保持非 detached，交给 taskkill /T 处理。
      detached: shouldSpawnInDetachedProcessGroup(),
      env: {
        ...sanitizeESCodeRuntimeEnv(process.env),
        [ESCODE_RUNTIME_ENV_KEY]: runtimeEnv,
        ...spawnEnv,
        ...effectiveCommand.env,
        // 生成文件根目录由执行端 Host 唯一决定，保证 Agent 写入与 Host 读取授权一致。
        [GEN_UI_OUTPUT_ROOT_ENV]: join(getAppConfigDir(), GEN_UI_OUTPUT_DIRECTORY),
        // 身份/隔离语义使用 workspaceIdentity；cwd 继续使用 workspacePath。
        ...buildAgentWorkspaceIdentityEnv(params.workspaceIdentity),
        ...buildE2EAgentCoverageEnv(),
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    const startedAt = Date.now();
    const stderrTail = createAgentStderrTail();
    const transport = new ESCodeStdioTransport(child, {
      onStderrLine: (line) => {
        const diagnostic = parseESCodeProcessDiagnostic(line);
        if (diagnostic && typeof child.pid === "number") {
          // 根因：只保存 exit tail 会漏掉存活 runtime 的异常；此旁路不依赖 debug 开关。
          // 身份绑定创建时的 child，不能查当前 workspace，避免重启后的迟到事件串进程。
          this.reportProcessLifecycle((reporter) =>
            reporter.onException?.({
              pid: child.pid!,
              provider: ESCODE_AGENT_PROVIDER,
              ...(this.lane ? { lane: this.lane } : {}),
              workspacePath: params.workspacePath,
              runtimeGeneration,
              runtimeInstanceId,
              diagnostic: {
                ...diagnostic,
                // 脱敏占位符可能比原文长，必须再次限长，避免 IPC schema 拒绝合法异常。
                name: redactAgentDiagnostic(diagnostic.name).slice(
                  0,
                  ESCODE_PROCESS_DIAGNOSTIC_NAME_MAX_CHARS,
                ),
                message: redactAgentDiagnostic(diagnostic.message).slice(
                  0,
                  ESCODE_PROCESS_DIAGNOSTIC_MESSAGE_MAX_CHARS,
                ),
                ...(diagnostic.stack !== undefined
                  ? {
                      stack: redactAgentDiagnostic(diagnostic.stack).slice(
                        0,
                        ESCODE_PROCESS_DIAGNOSTIC_STACK_MAX_CHARS,
                      ),
                    }
                  : {}),
              },
            }),
          );
          return;
        }
        stderrTail.append(line);
        debugLog(line);
      },
      ownedProcessStartedAtMs: spawnRequestedAt,
      // POSIX 下 child 由本 manager 以 detached=true 启动，pid 同时就是 Host
      // 拥有的独立 PGID；异常 root exit 后 cleanup 仍可按组回收同组后代。
      ...(process.platform !== "win32" && child.pid ? { ownedProcessGroupId: child.pid } : {}),
    });
    const client = new ESCodeProtocolClient(transport, {
      requireStorageStartup: effectiveCommand.supportsStorageStartup,
      requestTimeoutMs: this.requestTimeoutMs,
    });
    if (child.pid) {
      const unregisterNetworkCapture = localRuntimeNetworkCapture.register(client, child.pid);
      child.once("close", unregisterNetworkCapture);
    }
    // Agent 进程重启后，Host 仍需要 runtime identity 区分新旧订阅和运行命令。
    // Provider Registry 由新 Worker 从所属 Environment 的 Config 重建，不再由 UI 重新下发。
    const runtimeGeneration = (this.runtimeGenerationByWorkspaceKey.get(workspaceKey) ?? 0) + 1;
    this.runtimeGenerationByWorkspaceKey.set(workspaceKey, runtimeGeneration);
    // 生命周期事件关联只需要本次 runtime 的不透明身份，不能复用包含 workspaceKey 的协议 identity。
    const runtimeInstanceId = `agent-${randomUUID()}`;
    const runtimeIdentity: ESCodeAgentRuntimeIdentity = {
      generation: runtimeGeneration,
      identity: this.lane
        ? `${workspaceKey}:${runtimeGeneration}:${child.pid ?? "unknown"}:${this.lane}`
        : `${workspaceKey}:${runtimeGeneration}:${child.pid ?? "unknown"}`,
      ...(typeof child.pid === "number" ? { processId: child.pid } : {}),
      ...(this.lane ? { lane: this.lane } : {}),
      workspaceKey,
    };
    let resolveExit!: () => void;
    const exitPromise = new Promise<void>((resolve) => {
      resolveExit = resolve;
    });
    const managed: ManagedESCodeAgentProcess = {
      child,
      client,
      exited: false,
      exitPromise,
      resolveExit,
      readyReported: false,
      runtimeIdentity,
      runtimeInstanceId,
      spawned: false,
      startedAt,
      workspace: {
        workspacePath: params.workspacePath,
        ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
      },
    };
    this.processesByWorkspaceKey.set(workspaceKey, managed);
    this.ownedProcesses.add(managed);
    const publishStorage = () => {
      if (this.processesByWorkspaceKey.get(workspaceKey) !== managed) return;
      if (client.storageStartup.isWaiting) this.clearIdleTimer(managed);
      else if (
        client.storageStartup.snapshot?.phase === "ready" &&
        client.pendingOperationRequestCount === 0
      ) {
        this.scheduleIdleReclaim(workspaceKey, managed);
      }
      this.storageStartupEmitter.fire({
        workspaceKey,
        snapshot: { generation: runtimeGeneration, state: client.storageStartup.snapshot ?? null },
      });
    };
    client.storageStartup.onDidChange(publishStorage);
    publishStorage();
    if (this.idleTimeoutMs) {
      client.onPendingRequestsDrained(() => this.scheduleIdleReclaim(workspaceKey, managed));
    }
    child.once("spawn", () => {
      managed.spawned = true;
      // Node spawn() 会先返回 ChildProcess，再异步报告 cwd/command ENOENT。
      // 旧代码在确认 spawn 成功前就发布 runtimeRestarted，订阅方随即重连并再次触发
      // 启动，最终形成失败启动 -> 假重启 -> 重连的自激风暴。只有 spawn 事件才表示
      // 新 CLI 运行时真实存在，可以安全通知 v4 订阅方重订。
      if (runtimeGeneration > 1 && this.processesByWorkspaceKey.get(workspaceKey) === managed) {
        this.runtimeRestartedEmitter.fire({ workspaceKey, runtimeIdentity });
      }
      if (this.processesByWorkspaceKey.get(workspaceKey) === managed) {
        this.availableRuntimeIdentityByWorkspaceKey.set(workspaceKey, runtimeIdentity.identity);
        this.runtimeLifecycleEmitter.fire({
          workspacePath: params.workspacePath,
          ...(params.workspaceIdentity ? { workspaceIdentity: params.workspaceIdentity } : {}),
          workspaceKey,
          runtimeIdentity,
          state: "available",
        });
      }
      if (typeof child.pid === "number") {
        this.reportProcessLifecycle((reporter) =>
          reporter.onSpawn({
            pid: child.pid!,
            provider: ESCODE_AGENT_PROVIDER,
            ...(this.lane ? { lane: this.lane } : {}),
            workspacePath: params.workspacePath,
            command: effectiveCommand.command,
            args: effectiveCommand.args ?? [],
            startedAt,
            runtimeGeneration,
            runtimeInstanceId,
          }),
        );
      }
      this.reportRuntimeReady(managed);
      log("ESCode agent process started", {
        workspaceKey,
        command: effectiveCommand.command,
        cwd: spawnPreflight.cwd,
        pid: child.pid,
        durationMs: Date.now() - startStartedAt,
      });
    });
    child.once("error", (error) => {
      errorLog(
        `ESCode agent process error${this.processLifecycleReporter?.onError ? ` ${ESCODE_AGENT_LIFECYCLE_LOG_MARKER}` : ""}`,
        {
          workspaceKey,
          pid: child.pid,
          runtimeIdentity: runtimeIdentity.identity,
          errorName: error.name,
          errorMessage: error.message,
          errorStack: error.stack,
          spawnPreflight,
        },
      );
      const errno = error as NodeJS.ErrnoException;
      this.reportProcessLifecycle((reporter) =>
        reporter.onError?.({
          pid: typeof child.pid === "number" ? child.pid : null,
          provider: ESCODE_AGENT_PROVIDER,
          ...(this.lane ? { lane: this.lane } : {}),
          workspacePath: params.workspacePath,
          command: effectiveCommand.command,
          args: effectiveCommand.args ?? [],
          errorName: error.name || "Error",
          ...(typeof errno.code === "string" ? { errorCode: errno.code } : {}),
          errorMessage: error.message,
          ...(error.stack ? { errorStack: error.stack } : {}),
          runtimeGeneration,
          runtimeInstanceId,
          occurredAt: Date.now(),
        }),
      );
      this.noteRustStartupFailure(effectiveCommand, managed, "spawn_error");
      if (child.pid == null) {
        this.ownedProcesses.delete(managed);
        managed.resolveExit();
      }
    });
    child.once("exit", async (code, signal) => {
      managed.exited = true;
      // 退出码先于任何 await 落账：startClient 的就绪等待与 waitForRetiringRuntimes 之后
      // 都按这里的事实归因（stdout EOF 可能先于 exit 事件，读取方会等待 exitPromise）。
      managed.exitInfo = { code, signal };
      this.clearIdleTimer(managed);
      const endedAt = Date.now();
      const terminationKind = managed.terminationIntent?.kind ?? "unexpected";
      if (terminationKind === "unexpected") {
        this.noteRustStartupFailure(effectiveCommand, managed, "exit_before_ready", code);
      }
      // owner 冲突是唯一需要启动退避的失败：锁的持有者是另一个存活进程，无节流的重试只会反复
      // spawn 再以 75 退出。退出码 75 只可能出现在取得 owner 锁之前，因此这里只按「未 ready + 75」
      // 记录，清除只发生在成功接管（storage ready）时。
      if (
        code === RUST_WORKSPACE_OWNED_EXIT_CODE &&
        client.storageStartup.snapshot?.phase !== "ready"
      ) {
        this.noteWorkspaceOwnedConflict(workspaceKey);
      }
      // 必须在回退判定之后放行：等待中的新一代 runtime 解析命令时要看到本次判定结果。
      managed.resolveExit();
      // 协议解析/stream 故障会先触发 protocol-close，再由 Host 用 SIGTERM
      // 回收仍存活的进程。若只透传主动 termination intent，desktop 只能看到最终信号，
      // 无法区分协议故障与受控退出；保留首次 cleanup 原因作为结构化根因。
      const terminationReason = managed.terminationIntent?.reason ?? managed.firstCleanupReason;
      // 协议已立即失效，但 exit 先于 stderr EOF；保留旧 runtime 闭包身份收齐最后诊断。
      await transport.waitForStderrDrain();
      const stderr = stderrTail.snapshot();
      const exitContext = {
        workspaceKey,
        pid: child.pid,
        runtimeIdentity: runtimeIdentity.identity,
        code,
        signal,
        terminationKind,
        terminationReason,
      };
      // 之前日志只有新的 "process started"，缺少旧 pid 的退出轨迹。
      // agent native crash 后 UI 只会看到 protocol close/Session is not active，无法判断是崩溃还是主动重启。
      log("ESCode agent process exited", exitContext);
      if (terminationKind === "unexpected") {
        // Agent 顶层异常只写 stderr 并以非零 code 退出；stderr 过去仅走开发态
        // debug，生产日志只剩 code=1，无法还原异常。不能只按非零 code 判断：signal crash
        // 和长期运行的 Agent 自行 exit 0 同样是非预期退出。
        // 已有独立生命周期事件，显式标记包装日志，避免 Electron 将其再计为 JS 异常。
        errorLog(
          `ESCode agent process exited unexpectedly${this.processLifecycleReporter ? ` ${ESCODE_AGENT_LIFECYCLE_LOG_MARKER}` : ""}`,
          {
            ...exitContext,
            stderr,
          },
        );
      }
      if (typeof child.pid === "number") {
        this.reportProcessLifecycle((reporter) =>
          reporter.onExit({
            pid: child.pid!,
            provider: ESCODE_AGENT_PROVIDER,
            ...(this.lane ? { lane: this.lane } : {}),
            workspacePath: params.workspacePath,
            exitCode: code,
            signal,
            endedAt,
            terminationKind,
            runtimeReady: managed.readyAt != null,
            ...(terminationReason ? { terminationReason } : {}),
            runtimeGeneration,
            runtimeInstanceId,
            uptimeMs: Math.max(0, endedAt - startedAt),
            stderrLineCount: stderr.lineCount,
            ...(terminationKind === "unexpected" && stderr.tail.length > 0
              ? { stderrTail: stderr.tail }
              : {}),
          }),
        );
      }
    });
    client.onRequestTimeout((event) => {
      if (this.processesByWorkspaceKey.get(workspaceKey) !== managed) {
        return;
      }
      if (event.method === "workspace/cancelGenerateText") {
        warnLog(
          "ESCode agent cancel notification timed out; keeping client (best-effort control plane)",
          {
            workspaceKey,
            method: event.method,
            requestId: event.requestId,
            timeoutMs: event.timeoutMs,
            pid: child.pid,
          },
        );
        return;
      }
      warnLog("ESCode agent request timed out; disposing stale protocol client", {
        workspaceKey,
        method: event.method,
        requestId: event.requestId,
        timeoutMs: event.timeoutMs,
        pid: child.pid,
      });
      this.processesByWorkspaceKey.delete(workspaceKey);
      this.reportRuntimeUnavailable(managed);
      // timeout 说明协议请求/响应链路已经不可信。旧实现只 reject 当前请求，
      // 但 child 仍未 exit，后续同 workspace 会继续复用坏 client 并反复超时。
      // 这里主动回收进程树，让下一次 getClient 重新拉起干净的 app-server。
      void this.cleanupManagedProcessWithRetry(
        managed,
        "request-timeout",
        "request-timeout",
        "request timeout",
      ).catch(() => undefined);
    });
    client.onClose(() => {
      const wasActiveClient = this.processesByWorkspaceKey.get(workspaceKey) === managed;
      log("ESCode agent protocol client closed", {
        workspaceKey,
        pid: child.pid,
        runtimeIdentity: runtimeIdentity.identity,
        wasActiveClient,
      });
      // Rust 就绪前失败的判定统一放在 exit 里（需要退出码区分 workspace 所有权冲突）。协议关闭先于 exit
      // 到达也不会让紧接着的重连解析到失败的 Rust：startClient 会先等待本进程退出（waitForRetiringRuntimes）。
      if (wasActiveClient) {
        this.processesByWorkspaceKey.delete(workspaceKey);
      }
      this.reportRuntimeUnavailable(managed);
      if (this.ownedProcesses.has(managed)) {
        // 根 child 的 exit 不等于同组 MCP 后代已退出。即使 protocol close
        // 来自根进程退出，也必须按原进程组完成幂等回收后才能释放 Host 所有权。
        void this.cleanupManagedProcessWithRetry(
          managed,
          "protocol-close",
          "protocol-close",
          "protocol close",
        ).catch(() => undefined);
      }
    });
    // 命令声明存储启动时，client 只有就绪后才可用：就绪前进程退出会在这里以归因后的错误抛出，
    // 而不是把注定失败的 client 交给调用方（docs/specs/rust-packaging.md「owner 冲突如实上报与启动退避」）。
    if (effectiveCommand.supportsStorageStartup) {
      await this.waitForStorageStartupOrThrow(managed);
    }
    return client;
  }

  async getRuntimeIdentity(params: {
    workspacePath: string;
    workspaceIdentity?: string;
  }): Promise<ESCodeAgentRuntimeIdentity> {
    const workspaceKey = resolveWorkspaceKey(params);
    const managed = this.processesByWorkspaceKey.get(workspaceKey);
    // runtime identity 是查询接口，旧实现却复用了启动型 getClient，
    // 导致 provider 保存等被动探测按 workspace 数量隐式 spawn Agent CLI。
    if (!managed || managed.exited || managed.child.killed) {
      throw new Error("ESCode agent runtime identity is unavailable.");
    }
    return managed.runtimeIdentity;
  }

  async canStart(params: {
    workspacePath: string;
    workspaceIdentity?: string;
  }): Promise<{ available: boolean; workspaceKey: string; reason?: string }> {
    const workspaceKey = resolveWorkspaceKey(params);
    try {
      const command = await this.commandResolver({
        ...params,
        ...(this.presentationSurface ? { presentationSurface: this.presentationSurface } : {}),
        workspaceKey,
      });
      return command
        ? { available: true, workspaceKey }
        : {
            available: false,
            workspaceKey,
            reason: "ESCODE_AGENT_SERVER_COMMAND is not configured",
          };
    } catch (error) {
      return {
        available: false,
        workspaceKey,
        reason: error instanceof Error ? error.message : String(error),
      };
    }
  }

  async disposeWorkspace(params: {
    workspacePath: string;
    workspaceIdentity?: string;
  }): Promise<void> {
    const workspaceKey = resolveWorkspaceKey(params);
    this.restartGenerationByWorkspaceKey.set(
      workspaceKey,
      (this.restartGenerationByWorkspaceKey.get(workspaceKey) ?? 0) + 1,
    );
    this.abortPendingStarts(workspaceKey);
    const managed = this.processesByWorkspaceKey.get(workspaceKey);
    this.processesByWorkspaceKey.delete(workspaceKey);
    // dispose 不能把尚未完成的 start promise 从追踪表中删掉。删除会让
    // recovery/UI 的下一次 getClient 再开一条 spawn，旧 promise 随后又可能越过异步
    // resolve 回写进程池，形成同一 workspace 的 spawn/dispose 风暴。代际检查会让旧
    // promise 在真正 spawn 前失败，finally 再按 promise identity 清理 map。
    if (managed) {
      // restartWorkspaceProcess 只应回收当前 workspace 的 agent。
      // 不能复用 disposeAll，否则会把整个 manager 标记为已关闭，后续首发/预热无法重新拉起。
      this.reportRuntimeUnavailable(managed);
      await this.cleanupManagedProcessWithRetry(
        managed,
        "workspace-dispose",
        "workspace-dispose-retry",
        "workspace dispose",
      );
    }
  }

  private abortPendingStarts(
    workspaceKey: string,
    reason = new Error("ESCode agent process start was cancelled."),
  ): void {
    const controllers = this.startAdmissionAbortControllersByWorkspaceKey.get(workspaceKey);
    if (!controllers) {
      return;
    }
    for (const controller of controllers) {
      controller.abort(reason);
    }
  }

  private abortAllPendingStarts(
    reason = new Error("ESCode agent process manager is disposed."),
  ): void {
    for (const workspaceKey of this.startAdmissionAbortControllersByWorkspaceKey.keys()) {
      this.abortPendingStarts(workspaceKey, reason);
    }
  }

  disposeAll(): void {
    this.disposed = true;
    this.storageStartupEmitter.dispose();
    this.abortAllPendingStarts();
    for (const managed of this.ownedProcesses) {
      this.recordTerminationIntent(managed, "manager-dispose");
      this.reportRuntimeUnavailable(managed);
      managed.client.dispose();
    }
    this.processesByWorkspaceKey.clear();
    this.startingByWorkspaceKey.clear();
  }

  async disposeAllAndWait(): Promise<void> {
    if (this.disposeAllInFlight) {
      return this.disposeAllInFlight;
    }
    this.disposed = true;
    this.storageStartupEmitter.dispose();
    this.abortAllPendingStarts();

    const managedProcesses = [...this.ownedProcesses];
    for (const managed of managedProcesses) {
      this.reportRuntimeUnavailable(managed);
    }
    this.processesByWorkspaceKey.clear();
    this.startingByWorkspaceKey.clear();

    // app/host 退出时旧逻辑只同步 dispose client，底层进程树的 SIGKILL 兜底
    // 依赖 unref timer，host 自己退出后 timer 不会再执行，escode-cli 会残留为孤儿进程。
    // 这里让 host 可以等待每个 workspace 的 agent 进程树完成 graceful + force 清理。
    this.disposeAllInFlight = Promise.all(
      managedProcesses.map((managed) => this.cleanupManagedProcessForShutdown(managed)),
    ).then(() => undefined);
    try {
      await this.disposeAllInFlight;
    } finally {
      this.disposeAllInFlight = undefined;
    }
  }
}
