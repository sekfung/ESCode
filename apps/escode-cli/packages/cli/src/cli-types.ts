import type { TuiReadClipboardImage, TuiWriteClipboardText } from "@escode/tui";
import type { UiLocale } from "@escode/i18n";
import type { Logger } from "@escode/contracts";
import type {
  createManagedCdpBrowserRuntime,
  ManagedCdpBrowserRuntimeOptions,
} from "@escode/adapters/browser";
import type {
  createDefaultProviderEndpointRoutingPort,
  createModelAdapter,
<<<<<<< HEAD:apps/escode-cli/packages/cli/src/cli-types.ts
  createESCodeApp,
=======
  createZCodeApp,
  CreateDefaultProviderEndpointRoutingPortOptions,
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/cli/src/cli-types.ts
  CreateModelAdapterOptions,
  configureCodingPlanApiKey,
  ConfigureCodingPlanApiKeyOptions,
  inspectESCodeSkill,
  inspectWorkspaceHookTrust,
  grantWorkspaceHookTrust,
  revokeWorkspaceHookTrustCli,
  inspectESCodeCustomCommand,
  InspectESCodeCustomCommandOptions,
  InspectESCodeSkillOptions,
  loginESCodeCli,
  loginBigmodelCodingPlan,
  LoginBigmodelCodingPlanOptions,
  LoginESCodeCliOptions,
  listESCodeCustomCommands,
  ListESCodeCustomCommandsOptions,
  loadESCodeCustomCommand,
  listESCodeSessions,
  listESCodeSkills,
  ListESCodeSessionsOptions,
  ListESCodeSkillsOptions,
  logoutESCodeCli,
  LogoutESCodeCliOptions,
  resolveLatestSession,
  ResolveLatestSessionOptions,
  RunESCodeProtocolAgentOptions,
  prepareESCodeTelemetryEnv,
  startProcessProviderRegistryRuntime,
  shutdownESCodeTelemetry,
  ESCodeAppOptions,
} from "@escode/bootstrap";
import type { CliEnv, DotenvLoadResult, LoadCliDotenvOptions } from "./env.js";
import type { PluginsCommandOverrides } from "./plugins-command.js";
import type { CliShutdownProcess } from "./shutdown.js";
import type { resolveWorkspaceGitBranch } from "./tui-workspace-git.js";

export type BootstrapModule = typeof import("@escode/bootstrap");

export interface RunDependencies extends PluginsCommandOverrides {
  protocolLifecycle?: RunESCodeProtocolAgentOptions["lifecycle"];
  protocolInput?: NodeJS.ReadableStream;
  createManagedCdpBrowserRuntime?: (
    options?: ManagedCdpBrowserRuntimeOptions,
  ) => ReturnType<typeof createManagedCdpBrowserRuntime>;
  createModelAdapter?: (
    options?: CreateModelAdapterOptions,
  ) => ReturnType<typeof createModelAdapter>;
<<<<<<< HEAD:apps/escode-cli/packages/cli/src/cli-types.ts
  createESCodeApp?: (
    options?: ESCodeAppOptions,
  ) => Awaited<ReturnType<typeof createESCodeApp>> | ReturnType<typeof createESCodeApp>;
=======
  createProviderEndpointRoutingPort?: (
    options: CreateDefaultProviderEndpointRoutingPortOptions,
  ) => ReturnType<typeof createDefaultProviderEndpointRoutingPort>;
  createZCodeApp?: (
    options?: ZCodeAppOptions,
  ) => Awaited<ReturnType<typeof createZCodeApp>> | ReturnType<typeof createZCodeApp>;
>>>>>>> aac4755666d09fdcd70272fcf063c077a639015f:apps/zcode-cli/packages/cli/src/cli-types.ts
  /**
   * Session-event shaper for --output-format stream-json. Defaults to the
   * bootstrap module's, which is also what the protocol server uses; injectable
   * so a caller that supplies its own `createESCodeApp` (tests, embedders) can
   * still stream, since the bootstrap module is not loaded on that path.
   */
  mapSessionEvent?: BootstrapModule["mapSessionEvent"];
  cwd?: () => string;
  env?: CliEnv;
  inspectSkill?: (options: InspectESCodeSkillOptions) => ReturnType<typeof inspectESCodeSkill>;
  inspectWorkspaceHookTrust?: typeof inspectWorkspaceHookTrust;
  grantWorkspaceHookTrust?: typeof grantWorkspaceHookTrust;
  revokeWorkspaceHookTrustCli?: typeof revokeWorkspaceHookTrustCli;
  inspectCustomCommand?: (
    options: InspectESCodeCustomCommandOptions,
  ) => ReturnType<typeof inspectESCodeCustomCommand>;
  loginESCodeCli?: (options?: LoginESCodeCliOptions) => ReturnType<typeof loginESCodeCli>;
  loginBigmodelCodingPlan?: (
    options?: LoginBigmodelCodingPlanOptions,
  ) => ReturnType<typeof loginBigmodelCodingPlan>;
  configureCodingPlanApiKey?: (
    options: ConfigureCodingPlanApiKeyOptions,
  ) => ReturnType<typeof configureCodingPlanApiKey>;
  loadDotenv?: (options?: LoadCliDotenvOptions) => DotenvLoadResult;
  prepareESCodeTelemetryEnv?: typeof prepareESCodeTelemetryEnv;
  projectConfigPath?: string;
  listSessions?: (options: ListESCodeSessionsOptions) => ReturnType<typeof listESCodeSessions>;
  listCustomCommands?: (
    options: ListESCodeCustomCommandsOptions,
  ) => ReturnType<typeof listESCodeCustomCommands>;
  loadCustomCommand?: (
    options: InspectESCodeCustomCommandOptions,
  ) => ReturnType<typeof loadESCodeCustomCommand>;
  // headless slash 路由要和 app facade 的保留名 gate 用同一个判据；默认取 bootstrap 的，
  // 注入点只为让单测不必拉起整个 bootstrap 模块。见 prompt-command.ts。
  isReservedSlashCommandName?: BootstrapModule["isReservedESCodeSlashCommandName"];
  listSkills?: (options: ListESCodeSkillsOptions) => ReturnType<typeof listESCodeSkills>;
  logger?: Logger;
  readClipboardImage?: TuiReadClipboardImage;
  writeClipboardText?: TuiWriteClipboardText;
  resolveLatestSession?: (
    options: ResolveLatestSessionOptions,
  ) => ReturnType<typeof resolveLatestSession>;
  resolveWorkspaceGitBranch?: typeof resolveWorkspaceGitBranch;
  logoutESCodeCli?: (options?: LogoutESCodeCliOptions) => ReturnType<typeof logoutESCodeCli>;
  runESCodeProtocolAgent?: (options?: RunESCodeProtocolAgentOptions) => Promise<void>;
  runTui?: typeof import("@escode/tui").runTui;
  skipUserConfig?: boolean;
  userConfigPath?: string;
  exitProcess?: (code: number) => void;
  shutdownCleanupTimeoutMs?: number;
  shutdownProcess?: CliShutdownProcess;
  startProcessProviderRegistryRuntime?: typeof startProcessProviderRegistryRuntime;
  shutdownESCodeTelemetry?: typeof shutdownESCodeTelemetry;
}

export type CliPermissionMode = "build" | "plan" | "edit" | "yolo" | "guarded";
export type CliRuntimeMode = CliPermissionMode | "auto";

export interface CliModeState {
  current?: CliRuntimeMode;
  override?: CliPermissionMode;
}

export interface CliTargetRequest {
  objective: string;
  replaceExisting: boolean;
}

export type ModeCapableApp = Awaited<ReturnType<typeof createESCodeApp>> & {
  getMode?: () => CliRuntimeMode;
  setLocale?: (locale: UiLocale) => Promise<{ locale: "en-US" | "zh-CN" }>;
  setMode?: (mode: CliRuntimeMode) => Promise<{ mode: CliRuntimeMode }>;
};

export interface CliResumeRequest {
  continueSession: boolean;
  resumeSessionId?: string;
}
