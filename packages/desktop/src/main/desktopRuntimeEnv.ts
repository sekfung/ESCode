/* eslint-disable max-lines -- desktop runtime/env 解析需要集中维护 main/host/remote assets 的启动边界，拆分会扩大远程连接回归面。 */
import { existsSync, readFileSync } from "node:fs";
import { homedir, version as osVersion } from "node:os";
import { join, resolve, win32 } from "node:path";
import type { ConnectOptions } from "@escode/server/remote";
import { listSSHConfigAliasesFromLocalConfig } from "@escode/services/node";
import { DEV_HELPER_APP_NAME, HELPER_APP_NAME } from "@escode/escode-cua/broker/helperConstants";
import {
  ESCODE_APP_VERSION_ENV,
  ESCODE_BUILD_COMMIT_ID_ENV,
  ESCODE_AGENT_RUNTIME,
  ESCODE_COMMIT,
  ESCODE_DYNAMIC_WORKFLOW_MODE_ENV,
  ESCODE_ENV,
  ESCODE_PRODUCT_FLAVOR,
  ESCODE_RUNTIME_ENV_KEY,
  ESCODE_VERSION,
  buildESCodeToolEnvPassthroughEnv,
  resolveRuntimeESCodeEndpointOrigin,
  resolveZaiBusinessBaseUrl,
  resolveZaiOAuthClientId,
  resolveZaiOAuthOrigin,
  normalizeDynamicWorkflowMode,
  readESCodeAgentTelemetryEnv,
  sanitizeESCodeRuntimeEnv,
  type ESCodeRuntimeEnv,
} from "@escode/shared";
import { resolvePlatformKeyForPackagedApp } from "../../scripts/target-platform.mjs";
import {
  getAppConfigDir,
  getDataBaseDir,
  ESCODE_CUA_BUNDLED_HELPER_APP_PATH_ENV,
  ESCODE_WINDOWS_APP_INSTALL_DIR_ENV,
} from "@escode/services/node";
import {
  resolveRemoteCdnBaseUrls as resolveOrderedRemoteCdnBaseUrls,
  type ResolveRemoteCdnOptions,
} from "./remoteCdn.js";
import { getElectronAppPath, isElectronAppPackaged } from "./desktopElectronApp.js";

declare const __ESCODE_PACKAGED_AGENT_TELEMETRY_ENV__: Readonly<Record<string, string>> | undefined;

const isLocalDevelopmentRuntime = !isElectronAppPackaged();
export const desktopRuntimeEnv: ESCodeRuntimeEnv = isLocalDevelopmentRuntime
  ? "development"
  : "production";
// 身份看编译期 flavor 而不是 ESCODE_ENV：ESCODE_PREVIEW_IDENTITY=1 的生产后端构建同样是 Preview，
// 需要独立的应用名、Electron 数据目录和 Helper 安装子目录才能与正式版并排运行。
export const isPreviewPackagedRuntime =
  !isLocalDevelopmentRuntime && ESCODE_PRODUCT_FLAVOR === "preview";

function readRuntimeEnvOverride(name: string): string | undefined {
  return process.env[name]?.trim() || undefined;
}

function isTruthyRuntimeEnvOverride(name: string): boolean {
  const value = readRuntimeEnvOverride(name)?.toLowerCase();
  return value === "1" || value === "true" || value === "yes" || value === "on";
}

// e2e 运行的是生产构建，默认会和本机正式版 ESCode 共用 app name / userData，
// 触发 Electron 单实例锁后只激活已有窗口，Chromedriver 无法接管测试进程。
// 这里允许测试显式隔离运行时身份，正常桌面/远程 workspace路径保持原来的默认值。
export const runtimeApplicationName =
  readRuntimeEnvOverride("ESCODE_DESKTOP_APPLICATION_NAME") ??
  (isLocalDevelopmentRuntime
    ? "ESCode Dev"
    : isPreviewPackagedRuntime
      ? "ESCode Preview"
      : "ESCode");
// Electron 的 app.getPath("home") 不一定跟随测试进程里的 HOME 覆盖。
// e2e 默认工作区依赖 home 路径，因此提供显式覆盖，避免测试写到开发者真实 ~/ESCodeProject。
export const runtimeHomePath = readRuntimeEnvOverride("ESCODE_DESKTOP_HOME_DIR");
// Chromedriver 管理 Electron 时会注入临时 userData；e2e 默认路径模式下导入期不能提前读取 appData。
export const shouldUseElectronDefaultUserDataPath = isTruthyRuntimeEnvOverride(
  "ESCODE_DESKTOP_USE_ELECTRON_DEFAULT_USER_DATA",
);
export const runtimeUserDataPath =
  readRuntimeEnvOverride("ESCODE_DESKTOP_USER_DATA_DIR") ??
  (shouldUseElectronDefaultUserDataPath
    ? undefined
    : join(getElectronAppPath("appData"), runtimeApplicationName));
export const runtimeSessionDataPath =
  readRuntimeEnvOverride("ESCODE_DESKTOP_SESSION_DATA_DIR") ??
  (runtimeUserDataPath ? join(runtimeUserDataPath, "session") : undefined);
// Chromedriver 会注入临时 --user-data-dir，并在该目录等待 DevToolsActivePort。
// e2e 如果再用 app.setPath 覆盖 userData/sessionData，端口文件会被写到另一个目录，
// 导致 Electron 已启动但 WebDriver session 一直创建失败。测试态打开该开关后保留 Chromedriver 的目录。
export const hostModulePath = join(import.meta.dirname, "../host/index.js");
export const schedulerModulePath = join(import.meta.dirname, "../scheduler/index.js");
export function getCredentialsDir() {
  return getAppConfigDir();
}

export type RemoteAssetDirs = Pick<
  ConnectOptions,
  "mockCdnDir" | "remoteCdnBaseUrl" | "remoteCdnBaseUrls" | "remoteCacheDir"
>;
type LocalRuntimeEnv = Record<string, string | undefined>;

export async function isDockerDaemonAvailable(): Promise<boolean> {
  const { isDockerAvailable } = await import("@escode/server/remote");
  return isDockerAvailable();
}

export async function listAvailableWSLDistros() {
  const { listWSLDistros } = await import("@escode/server/remote");
  return listWSLDistros();
}

export async function listAvailableDockerContainers() {
  const { listDockerContainers } = await import("@escode/server/remote");
  return listDockerContainers();
}

export async function listSSHConfigAliases() {
  return await listSSHConfigAliasesFromLocalConfig();
}

function parseDotenv(content: string): Record<string, string> {
  const values: Record<string, string> = {};

  for (const rawLine of content.split("\n")) {
    const line = rawLine.trim();
    if (line === "" || line.startsWith("#")) {
      continue;
    }

    const normalized = line.startsWith("export ") ? line.slice("export ".length).trim() : line;
    const equalsIndex = normalized.indexOf("=");
    if (equalsIndex <= 0) {
      continue;
    }

    const key = normalized.slice(0, equalsIndex).trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
      continue;
    }

    let value = normalized.slice(equalsIndex + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }

    values[key] = value;
  }

  return values;
}

function resolveWorkspaceRootForEnvFiles(): string | null {
  const workspaceRootCandidate = resolve(import.meta.dirname, "../../../..");
  return existsSync(join(workspaceRootCandidate, "pnpm-workspace.yaml"))
    ? workspaceRootCandidate
    : null;
}

export function loadHostProcessEnvFromLocalFiles(): Record<string, string> {
  if (isElectronAppPackaged()) {
    return resolvePackagedAgentTelemetryEnv();
  }

  const desktopRoot = resolve(import.meta.dirname, "../..");
  const workspaceRoot = resolveWorkspaceRootForEnvFiles();
  const fileCandidates = [
    ...(workspaceRoot
      ? [resolve(workspaceRoot, ".env"), resolve(workspaceRoot, ".env.local")]
      : []),
    // 开发态 host process 不经过 Vite，自行加载相同的 .env 文件以保持 OAuth 配置一致。
    ...(workspaceRoot && isLocalDevelopmentRuntime
      ? [
          resolve(workspaceRoot, ".env.development"),
          resolve(workspaceRoot, ".env.development.local"),
        ]
      : []),
    resolve(desktopRoot, ".env"),
    resolve(desktopRoot, ".env.local"),
    ...(isLocalDevelopmentRuntime
      ? [resolve(desktopRoot, ".env.development"), resolve(desktopRoot, ".env.development.local")]
      : []),
  ];

  const merged: Record<string, string> = {};
  const seen = new Set<string>();

  for (const candidate of fileCandidates) {
    if (seen.has(candidate)) {
      continue;
    }
    seen.add(candidate);

    if (!existsSync(candidate)) {
      continue;
    }

    // 之前按 cwd 向上级目录泛搜 .env，容易误读到工作区外的同名文件。
    // 这里将加载范围收敛为 workspace 根与 desktop 包目录，避免配置来源漂移。
    const parsed = parseDotenv(readFileSync(candidate, "utf-8"));
    Object.assign(merged, parsed);
  }

  return applySelectedESCodeEnvLinks(merged);
}

export function resolvePackagedAgentTelemetryEnv(
  compiledEnv:
    | Readonly<Record<string, string>>
    | undefined = typeof __ESCODE_PACKAGED_AGENT_TELEMETRY_ENV__ === "undefined"
    ? undefined
    : __ESCODE_PACKAGED_AGENT_TELEMETRY_ENV__,
): Record<string, string> {
  if (!compiledEnv) {
    return {};
  }

  // 修复原因：开发版通过启动进程继承 OTLP 配置，正式安装包从 Finder/开始菜单启动时没有
  // 这组运行时环境，Agent 因而静默落入 Noop。构建默认值只允许连接字段进入 Main；
  // UID、Device MID 和 runtime surface 仍由 Host 的可信运行时上下文生成。
  return readESCodeAgentTelemetryEnv({
    OTEL_EXPORTER_OTLP_ENDPOINT: compiledEnv.OTEL_EXPORTER_OTLP_ENDPOINT,
    OTEL_EXPORTER_OTLP_HEADERS: compiledEnv.OTEL_EXPORTER_OTLP_HEADERS,
    OTEL_SERVICE_NAME: compiledEnv.OTEL_SERVICE_NAME,
    ESCODE_TELEMETRY_RUNTIME_DISTRIBUTION: compiledEnv.ESCODE_TELEMETRY_RUNTIME_DISTRIBUTION,
  });
}

function resolveDevelopmentMockCdnDir(): string {
  return join(import.meta.dirname, "../../mock-cdn");
}

function resolveAvailableDevelopmentMockCdnDir(): string | undefined {
  const mockCdnDir = resolveDevelopmentMockCdnDir();
  const releaseDir = join(mockCdnDir, "releases", ESCODE_VERSION);
  // 开发态 mock-cdn 是可选离线缓存。当前版本目录不存在时继续传 mockCdnDir，
  // 会让 WSL/SSH 重连先命中一个必然缺失的本地路径，遮蔽已有的 CDN/cache fallback。
  return existsSync(releaseDir) ? mockCdnDir : undefined;
}

/**
 * 安装包内置远程资源的打包态判据。
 *
 * 安装包构建期已按构建目标架构从 CDN 下载对应的 Linux 远程 release（见
 * docs/specs/offline-remote-assets.md），落在 resources/remote-assets/releases/<version>/。
 * 运行时把它当作本地 release 源直接上传，使 SSH / WSL / Docker 首次连接不再访问 CDN。
 *
 * 未内置的远程平台（例如只带 linux-x64 却连接 darwin 远端）由 deploy 侧按组件回退 CDN。
 */
function resolveAvailableBundledRemoteAssetsDir(): string | undefined {
  const resourcesPath = process.resourcesPath;
  if (typeof resourcesPath !== "string" || resourcesPath.length === 0) {
    return undefined;
  }

  const bundledDir = join(resourcesPath, "remote-assets");
  const releaseDir = join(bundledDir, "releases", ESCODE_VERSION);
  // 与 mock-cdn 同一判据：App 版本目录缺失时不能暴露本地源，否则连接会先命中
  // 一个必然缺失的路径，遮蔽已有的 CDN/cache fallback。
  return existsSync(releaseDir) ? bundledDir : undefined;
}

function isTruthyEnvFlag(value: string | undefined): boolean {
  if (!value) {
    return false;
  }

  const normalized = value.trim().toLowerCase();
  return normalized === "1" || normalized === "true" || normalized === "yes" || normalized === "on";
}

function shouldUseRemoteCdnInDevelopment(localEnv: LocalRuntimeEnv = {}): boolean {
  return isTruthyEnvFlag(resolveEnvValue("ESCODE_DEV_REMOTE_ASSET_USE_CDN", localEnv));
}

function resolveRemoteCdnBaseUrls(
  options: ResolveRemoteCdnOptions = {},
  localEnv: LocalRuntimeEnv = {},
): string[] {
  const raw = resolveEnvValue("ESCODE_REMOTE_ASSET_CDN_BASE_URL", localEnv);
  return resolveOrderedRemoteCdnBaseUrls({
    ...options,
    env: ESCODE_ENV,
    overrideBaseUrl: raw,
    version: ESCODE_VERSION,
  });
}

function resolveEnvValue(envName: string, localEnv: LocalRuntimeEnv = {}): string | undefined {
  return process.env[envName]?.trim() || localEnv[envName]?.trim() || undefined;
}

export function resolveESCodeEndpointEnvBaseOrigin(
  localEnv: LocalRuntimeEnv = {},
): string | undefined {
  const scopedEnvName =
    ESCODE_ENV === "production" ? "ESCODE_PRODUCTION_BASE_URL" : "ESCODE_TEST_BASE_URL";
  // Bugfix: main 进程临时验证更新服务时不会重新写 .env，命令行传入的 endpoint 必须优先于本地文件。
  return (
    process.env["ESCODE_BASE_URL"]?.trim() ||
    process.env["ESCODE_ENDPOINT_ORIGIN"]?.trim() ||
    process.env[scopedEnvName]?.trim() ||
    localEnv.ESCODE_BASE_URL?.trim() ||
    localEnv.ESCODE_ENDPOINT_ORIGIN?.trim() ||
    localEnv[scopedEnvName]?.trim() ||
    undefined
  );
}

function readDefinedProcessEnv(): Record<string, string> {
  const values: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (typeof value === "string") {
      values[key] = value;
    }
  }
  return values;
}

function applySelectedESCodeEnvLinks(env: Record<string, string>): Record<string, string> {
  const endpointEnv = {
    ...env,
    ESCODE_ENV,
  };

  return {
    ...env,
    ESCODE_BASE_URL: env.ESCODE_BASE_URL ?? resolveRuntimeESCodeEndpointOrigin(endpointEnv),
    ZAI_OAUTH_ORIGIN: env.ZAI_OAUTH_ORIGIN ?? resolveZaiOAuthOrigin(endpointEnv),
    ZAI_BUSINESS_BASE_URL: env.ZAI_BUSINESS_BASE_URL ?? resolveZaiBusinessBaseUrl(endpointEnv),
    ZAI_OAUTH_CLIENT_ID: env.ZAI_OAUTH_CLIENT_ID ?? resolveZaiOAuthClientId(endpointEnv),
  };
}

function resolveHostProcessNodeEnv(): ESCodeRuntimeEnv {
  return desktopRuntimeEnv;
}

function resolveRemoteAssetCacheDir(localEnv: LocalRuntimeEnv = {}): string {
  const overrideCacheDir = resolveEnvValue("ESCODE_REMOTE_ASSET_CACHE_DIR", localEnv);
  if (overrideCacheDir) {
    // 开发态需要复用正式版 remote cache 验证下载判断，但不能整体切换 Electron userData。
    // 因此只允许覆盖 remote assets cache 目录，避免污染登录态、窗口状态等其它开发数据。
    return resolve(overrideCacheDir);
  }

  return join(getElectronAppPath("userData"), "remote-assets-cache");
}

export function resolveRemoteAssetDirs(
  options: ResolveRemoteCdnOptions = {},
  localEnv: LocalRuntimeEnv = {},
): RemoteAssetDirs {
  const remoteCdnBaseUrls = resolveRemoteCdnBaseUrls(options, localEnv);
  const remoteCdnBaseUrl = remoteCdnBaseUrls[0];

  // remote 资源和 desktop 本地 provider 资源曾共用安装包内路径，打包后会把整套 Linux
  // 远程运行时塞进 .app。现在按用途显式分流：
  // - 开发态：只读仓库里的 mock-cdn（默认），显式开关才切到公网 CDN 验证真实下载链路；
  // - 打包态：读取安装包按构建目标架构内置的 remote-assets（离线免下载），
  //   其余远程平台仍走 CDN + 本地缓存目录。
  // 两条本地源都只在当前 App 版本目录存在时才暴露，缺失时保持原有 CDN/cache 回退。
  if (isElectronAppPackaged() || shouldUseRemoteCdnInDevelopment(localEnv)) {
    // 内置远程资源的安装包同样暴露本地 release 源：离线时免下载，
    // 该平台未内置或本地资源不完整时由 deploy 侧回退 CDN/cache。
    const bundledRemoteAssetsDir = isElectronAppPackaged()
      ? resolveAvailableBundledRemoteAssetsDir()
      : undefined;
    return {
      ...(bundledRemoteAssetsDir ? { mockCdnDir: bundledRemoteAssetsDir } : {}),
      remoteCdnBaseUrl,
      remoteCdnBaseUrls,
      remoteCacheDir: resolveRemoteAssetCacheDir(localEnv),
    };
  }

  const developmentMockCdnDir = resolveAvailableDevelopmentMockCdnDir();
  return {
    ...(developmentMockCdnDir ? { mockCdnDir: developmentMockCdnDir } : {}),
    remoteCdnBaseUrl,
    remoteCdnBaseUrls,
    remoteCacheDir: resolveRemoteAssetCacheDir(localEnv),
  };
}

function resolveBundledESCodeAgentBinaryPath(): string | undefined {
  return resolveBundledESCodeAgentResourcePath(
    ESCODE_AGENT_RUNTIME.resolveEntrySegments(process.platform),
  );
}

function resolveBundledESCodeAgentResourcePath(entrySegments: string[]): string | undefined {
  const runtime = ESCODE_AGENT_RUNTIME;
  const platformKey = resolvePlatformKeyForPackagedApp();
  const candidates = [
    isElectronAppPackaged()
      ? join(process.resourcesPath, runtime.bundledResourceDir, ...entrySegments)
      : null,
    // desktop 开发态的启动 cwd 可能是 packages/desktop，也可能是仓库根，
    // 之前这里只按 import.meta.dirname 的相对路径推导 bundled-agents，
    // 这里补齐和 services 侧一致的多候选根目录，避免开发/构建/重启入口不同导致资源解析漂移。
    join(
      process.cwd(),
      "bundled-agents",
      platformKey,
      runtime.bundledResourceDir,
      ...entrySegments,
    ),
    join(
      process.cwd(),
      "packages",
      "desktop",
      "bundled-agents",
      platformKey,
      runtime.bundledResourceDir,
      ...entrySegments,
    ),
    join(
      import.meta.dirname,
      "../../bundled-agents",
      platformKey,
      runtime.bundledResourceDir,
      ...entrySegments,
    ),
  ].filter((candidate): candidate is string => Boolean(candidate));

  return candidates.find((candidate) => existsSync(candidate));
}

function resolveBundledRuntimeToolBinaryPath(
  toolDir: string,
  binaryName: string,
): string | undefined {
  const candidateBinaryName = process.platform === "win32" ? `${binaryName}.exe` : binaryName;
  const candidates = [
    isElectronAppPackaged()
      ? join(process.resourcesPath, "tools", toolDir, candidateBinaryName)
      : null,
    join(
      import.meta.dirname,
      "../../bundled-tools",
      resolvePlatformKeyForPackagedApp(),
      toolDir,
      candidateBinaryName,
    ),
  ].filter((candidate): candidate is string => Boolean(candidate));

  return candidates.find((candidate) => existsSync(candidate));
}

function resolveBundledLarkCliBinaryPath(): string | undefined {
  return resolveBundledRuntimeToolBinaryPath("lark-cli", "lark-cli");
}

export function resolveBundledGlmBinaryPath(): string | undefined {
  return resolveBundledESCodeAgentBinaryPath();
}

/**
 * 安装包实际随附的 Agent runtime：Rust 二进制与 Electron Node 执行的 zcode.cjs。
 * Bugfix：启动诊断此前只查旧原生 escode-agent（GLM_BINARY_PATH 语义），它已不再随包，
 * 每次启动都误报 "missing providers: glm" / "glm binary path: <not found>"，干扰排障。
 */
export function resolveBundledAgentRuntimePaths(): {
  rustBinary: string | undefined;
  nodeBundle: string | undefined;
} {
  return {
    rustBinary: resolveBundledESCodeAgentResourcePath(
      ESCODE_AGENT_RUNTIME.resolveRustBinarySegments(process.platform),
    ),
    nodeBundle: resolveBundledESCodeAgentResourcePath(
      ESCODE_AGENT_RUNTIME.resolveNodeBundleSegments(),
    ),
  };
}

function resolveHostProcessBinaryEnv(
  envVar: string,
  hostProcessLocalEnv: Record<string, string>,
  bundledPath: string | undefined,
): string | undefined {
  // ESCode Agent 与 app 协议适配强绑定版本，生产包必须优先使用随包携带的固定 runtime。
  // 用户机器或本地 .env 里残留的 GLM_BINARY_PATH 即使存在，也可能版本不兼容。
  // 只有 bundled runtime 缺失时才把显式路径作为兜底，避免用户本机 CLI 覆盖内嵌版本。
  if (bundledPath) {
    return bundledPath;
  }
  const explicitPath = process.env[envVar]?.trim() || hostProcessLocalEnv[envVar]?.trim();
  if (explicitPath && existsSync(explicitPath)) {
    return explicitPath;
  }
  return undefined;
}

export function resolveWindowsAppInstallDirForDataBaseDirGuard(
  options: {
    platform?: NodeJS.Platform | string;
    isPackaged?: boolean;
    resourcesPath?: string;
  } = {},
): string | undefined {
  const platform = options.platform ?? process.platform;
  const packaged = options.isPackaged ?? isElectronAppPackaged();
  const resourcesPath = options.resourcesPath ?? process.resourcesPath;
  if (platform !== "win32" || !packaged) {
    return undefined;
  }

  const trimmedResourcesPath = resourcesPath?.trim();
  if (!trimmedResourcesPath) {
    return undefined;
  }

  return win32.dirname(trimmedResourcesPath);
}

/**
 * Dynamic Workflow 灰度的本地覆盖按构建档位分三层
 *
 *   - 未打包 dev：透传 shell 里的合法取值，方便手工切档；非法值直接丢弃而不是转发给 Host，
 *     Host 因此不必再判一次来源；
 *   - 打包 preview：固定写入 `alwaysOn`，忽略 shell，preview 用户始终拥有该功能；
 *   - 打包 production：不写入，且继承值必须被删除，否则本机环境变量就能自行打开灰度。
 * Main 是唯一决策者：对这个键只有「写」和「删」两种动作，绝不原样透传，
 * Host 端的 resolveDynamicWorkflowClientConfig 才能无条件相信读到的值。
 */
export function resolveDynamicWorkflowModeHostEnv(options: {
  inheritedValue: string | undefined;
  isPackaged: boolean;
  isPreview: boolean;
}): Record<string, string> {
  if (!options.isPackaged) {
    const mode = normalizeDynamicWorkflowMode(options.inheritedValue);
    return mode ? { [ESCODE_DYNAMIC_WORKFLOW_MODE_ENV]: mode } : {};
  }
  if (options.isPreview) {
    return { [ESCODE_DYNAMIC_WORKFLOW_MODE_ENV]: "alwaysOn" };
  }
  return {};
}

export function buildHostProcessEnv(hostProcessLocalEnv: Record<string, string>) {
  const glmBinaryPath = resolveBundledGlmBinaryPath();
  const larkCliBinaryPath = resolveBundledLarkCliBinaryPath();
  const resolvedGlmBinaryPath = resolveHostProcessBinaryEnv(
    "GLM_BINARY_PATH",
    hostProcessLocalEnv,
    glmBinaryPath,
  );
  const resolvedLarkCliBinaryPath = resolveHostProcessBinaryEnv(
    "ESCODE_LARK_CLI_BINARY",
    hostProcessLocalEnv,
    larkCliBinaryPath,
  );
  const dataBaseDir = getDataBaseDir();
  const rawInheritedEnv = {
    ...hostProcessLocalEnv,
    ...readDefinedProcessEnv(),
  };
  const packagedDesktop = isElectronAppPackaged();
  const bundledCuaHelperAppPath =
    process.platform !== "darwin"
      ? undefined
      : packagedDesktop
        ? join(process.resourcesPath, "cua-helper", HELPER_APP_NAME)
        : // Truthy grammar must match the producer's isUnsignedHelperLocalDevRequested
          // (1|true|on, case-insensitive). Accepting only the literal "1" silently
          // ignored `true`/`on` set by scripts following the documented dev flow.
          ["1", "true", "on"].includes(
              rawInheritedEnv.ESCODE_CUA_HELPER_ALLOW_UNSIGNED_LOCAL?.trim().toLowerCase() ?? "",
            )
          ? rawInheritedEnv.ESCODE_CUA_BUNDLED_HELPER_APP_PATH?.trim() ||
            join(
              rawInheritedEnv.ESCODE_HOME?.trim() || join(homedir(), ".escode"),
              "computer-use",
              "dev",
              DEV_HELPER_APP_NAME,
            )
          : undefined;
  const windowsAppInstallDir = resolveWindowsAppInstallDirForDataBaseDirGuard();
  const agentTelemetryEnv = readESCodeAgentTelemetryEnv(rawInheritedEnv);
  // Desktop 身份由 host 从凭据仓库和本机状态读取后可信注入；外部环境只能配置 OTLP 连接，
  // 不能伪造 uid/device/runtime surface 或绕过本地 identity state 的隔离边界。
  for (const key of [
    "ESCODE_TELEMETRY_USER_ID",
    "ESCODE_TELEMETRY_USER_ID_HASH",
    "ESCODE_TELEMETRY_USER_SUBJECT_ID",
    "ESCODE_TELEMETRY_IDENTITY_STATE",
    "ESCODE_TELEMETRY_DEVICE_MID",
    "ESCODE_TELEMETRY_RUNTIME_SURFACE",
  ]) {
    delete agentTelemetryEnv[key];
  }
  const inheritedEnv = applySelectedESCodeEnvLinks({
    ...sanitizeESCodeRuntimeEnv(rawInheritedEnv),
    ...buildESCodeToolEnvPassthroughEnv(rawInheritedEnv),
  });
  // A release app must never inherit the local unsigned-Helper escape hatch.
  // Otherwise a developer shell/launchctl variable can make the signed app
  // reject its verified bundled Helper and route onboarding to a stale dev app.
  if (packagedDesktop) {
    delete inheritedEnv.ESCODE_CUA_HELPER_ALLOW_UNSIGNED_LOCAL;
  }
  const dynamicWorkflowModeHostEnv = resolveDynamicWorkflowModeHostEnv({
    inheritedValue: rawInheritedEnv[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV],
    isPackaged: packagedDesktop,
    isPreview: isPreviewPackagedRuntime,
  });
  // 三层里有两层不写这个键，空对象无法覆盖 inheritedEnv，所以先无条件删掉继承值再按决策 spread 回去。
  // 少了这一行，production 包和 dev 的非法取值都会原样穿透到 Host。
  delete inheritedEnv[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV];

  return {
    ...inheritedEnv,
    // OTLP 凭据只定向传到 host；host 初始化 services 时会立即捕获并从 process.env 清除，
    // 后续只在启动 Agent 时短暂注入，不会进入 Bash/MCP/tool env。
    ...agentTelemetryEnv,
    // ESCode 运行时不再使用 NODE_ENV；它会被用户 shell、包管理器和测试框架复用。
    // 这里显式下发 ESCODE_RUNTIME_ENV，并在继承环境里清掉 NODE_ENV，避免 host/agent/Bash 被污染。
    [ESCODE_RUNTIME_ENV_KEY]: resolveHostProcessNodeEnv(),
    // 显式注入编译期产品身份，保证主进程与 host 的身份语义一致；地址独立解析。
    // inheritedEnv 从 .env 通用变量补齐 ESCode/ZAI 链接，未覆盖时统一使用线上默认值。
    ESCODE_ENV,
    // Preview 与生产版共享任务、配置和凭据，但不同版本的 Helper 不能互相覆盖或触发降级保护。
    // 只隔离 computer-use 下的运行组件，不改写 ESCODE_HOME / ESCODE_DATA_BASE_DIR 业务数据根。
    ...(isPreviewPackagedRuntime ? { ESCODE_CUA_HELPER_INSTALL_VARIANT: "preview" } : {}),
    // Dynamic Workflow 灰度的本地覆盖：Main 决策后写入，production 包为空对象（继承值已在上面删除）。
    ...dynamicWorkflowModeHostEnv,
    // 模型请求默认 header 由 agent 进程构造，过去只继承 shell env 导致桌面启动时拿不到 app 版本。
    // 这里从 main 进程显式下发，agent 子进程继承 host env 后即可稳定写入请求 header。
    [ESCODE_APP_VERSION_ENV]: ESCODE_VERSION,
    // Telemetry 的构建身份必须来自 Desktop 编译期元数据，不能让 Agent 在运行时读取
    // 工作树或根据安装路径猜测；否则同一产品版本的不同修复包无法在 ARMS 中区分。
    [ESCODE_BUILD_COMMIT_ID_ENV]: ESCODE_COMMIT,
    ...(dataBaseDir !== homedir() ? { ESCODE_DATA_BASE_DIR: dataBaseDir } : {}),
    ...(windowsAppInstallDir ? { [ESCODE_WINDOWS_APP_INSTALL_DIR_ENV]: windowsAppInstallDir } : {}),
    ...(bundledCuaHelperAppPath
      ? { [ESCODE_CUA_BUNDLED_HELPER_APP_PATH_ENV]: bundledCuaHelperAppPath }
      : {}),
    ...(resolvedGlmBinaryPath ? { GLM_BINARY_PATH: resolvedGlmBinaryPath } : {}),
    ...(resolvedLarkCliBinaryPath ? { ESCODE_LARK_CLI_BINARY: resolvedLarkCliBinaryPath } : {}),
  };
}

/**
 * Rust Host 嵌入运行时的环境（rust-u2-host-skeleton.md §6，审计 A16）：与 Node Host 进程同一份基础 env
 * （数据根、运行环境标识、版本/提交、内置二进制路径、工具透传……），但去掉 OTLP 连接配置——
 * 它只允许在启动 Agent 时短暂注入，不能进入实例 env 快照（快照会被工具/MCP 子进程继承）。
 */
export function buildRustHostRuntimeEnv(
  hostProcessLocalEnv: Record<string, string>,
): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(buildHostProcessEnv(hostProcessLocalEnv))) {
    if (typeof value === "string") env[key] = value;
  }
  const telemetryKeys = Object.keys(
    readESCodeAgentTelemetryEnv({ ...hostProcessLocalEnv, ...readDefinedProcessEnv() }),
  );
  for (const key of telemetryKeys) delete env[key];
  return env;
}

/**
 * Rust Host 的 provider 配置位置（rust-u2-providers.md §11/§12，审计 A16b）：Bundled/Personal 文件路径、
 * Built-in Active/LKG 缓存的定位（环境配置根、应用版本）与 Endpoint 的输入。
 * Endpoint = `resolveESCodeEndpointOrigin({env, envBaseOrigin, overrideOrigin})`：Main 只交 `env` 与 env 基址，
 * 设置项 `escodeEndpointOrigin`（覆盖值）由 Host 读它自己持有的设置文件，所以切换 Endpoint 不需要额外的控制消息。
 * `ESCODE_BUILTIN_REMOTE_REFRESH=1`：由 Main 显式声明 Host 才下载远端 Release。
 */
export function buildRustHostProviderEnv(options: {
  readonly builtinFilePath: string;
  readonly appConfigDir: string;
  readonly appVersion: string;
  readonly escodeEnv: string;
  readonly endpointBaseOrigin?: string;
}): Record<string, string> {
  return {
    ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: options.builtinFilePath,
    ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: join(options.appConfigDir, "provider_config.json"),
    ESCODE_BUILTIN_CACHE_ROOT: options.appConfigDir,
    ESCODE_APP_VERSION: options.appVersion,
    ESCODE_ENV: options.escodeEnv,
    ...(options.endpointBaseOrigin ? { ESCODE_ENDPOINT_ORIGIN: options.endpointBaseOrigin } : {}),
    ESCODE_BUILTIN_REMOTE_REFRESH: "1",
  };
}

/**
 * Rust Host 的 ApiClient 来源头里 Host 自己拿不到的事实（rust-u2-client-config.md §3）：
 * Host 没有 `Intl`/`os.version()`，由 Main 经 `runtimeEnv` 下发。值缺省或非可打印 ASCII 时 Host 自行回落到 `unknown`/不带头。
 */
export function buildRustHostClientEnv(
  facts: {
    readonly language?: string;
    readonly timezone?: string;
    readonly osVersion?: string;
  } = {
    language: Intl.DateTimeFormat().resolvedOptions().locale,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    osVersion: osVersion(),
  },
): Record<string, string> {
  return {
    ...(facts.language ? { ESCODE_CLIENT_LANGUAGE: facts.language } : {}),
    ...(facts.timezone ? { ESCODE_CLIENT_TIMEZONE: facts.timezone } : {}),
    ...(facts.osVersion ? { ESCODE_OS_VERSION: facts.osVersion } : {}),
  };
}
