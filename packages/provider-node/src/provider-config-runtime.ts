import {
  ProviderConfigService,
  type ProviderConfigLayerSnapshot,
  type ProviderConfigLayerUpdate,
} from "@escode/provider";
import { NodeESCodeBuiltinProviderConfigSource } from "./escode-builtin-provider-config-source.js";
import {
  EndpointScopedESCodeBuiltinSource,
  type EndpointScopedESCodeBuiltinSourceOptions,
} from "./endpoint-scoped-escode-builtin-source.js";
import {
  ESCodeBuiltinRemoteSynchronizer,
  type ESCodeBuiltinRemoteSynchronizerOptions,
  type ESCodeBuiltinRefreshResult,
} from "./escode-builtin-remote-synchronizer.js";
import {
  NodePersonalProviderConfigRepository,
  type PersonalProviderConfigRecoveryEvent,
} from "./personal-provider-config-repository.js";

export interface NodeProviderConfigRuntimeOptions {
  readonly escodeBuiltinFilePath: string;
  readonly escodeBuiltinActiveFilePath?: string;
  readonly escodeBuiltinRemote?: Omit<ESCodeBuiltinRemoteSynchronizerOptions, "source">;
  readonly escodeBuiltinEnvironment?: Omit<
    EndpointScopedESCodeBuiltinSourceOptions,
    "bundledFilePath"
  >;
  readonly onESCodeBuiltinRefreshError?: (error: unknown) => void;
  readonly onPersonalConfigRecovery?: (event: PersonalProviderConfigRecoveryEvent) => void;
  readonly onPersonalConfigPollingError?: (error: unknown) => void;
  readonly personalFilePath: string;
  readonly personalPollingIntervalMs?: number | false;
  readonly importLegacy?: (
    escodeBuiltin: ProviderConfigLayerSnapshot,
  ) => Promise<ProviderConfigLayerUpdate | null>;
  readonly watch?: boolean;
}

/** 组装一个 Node.js 进程内共享的 ESCode Built-in/Personal Config 运行边界。 */
export class NodeProviderConfigRuntime {
  readonly configService: ProviderConfigService;
  readonly #escodeBuiltinSource:
    | NodeESCodeBuiltinProviderConfigSource
    | EndpointScopedESCodeBuiltinSource;
  readonly #personalRepository: NodePersonalProviderConfigRepository;
  readonly #remoteSynchronizer?: ESCodeBuiltinRemoteSynchronizer;
  readonly #onRemoteRefreshError?: (error: unknown) => void;
  #startPromise: Promise<void> | null = null;
  #disposed = false;
  readonly #checkListeners = new Set<() => Promise<void>>();
  #checkTimer: ReturnType<typeof setInterval> | null = null;
  #checkInFlight: Promise<void> | null = null;

  constructor(options: NodeProviderConfigRuntimeOptions) {
    this.#escodeBuiltinSource = options.escodeBuiltinEnvironment
      ? new EndpointScopedESCodeBuiltinSource({
          bundledFilePath: options.escodeBuiltinFilePath,
          ...options.escodeBuiltinEnvironment,
        })
      : new NodeESCodeBuiltinProviderConfigSource({
          bundledFilePath: options.escodeBuiltinFilePath,
          activeFilePath: options.escodeBuiltinActiveFilePath,
          watch: options.watch,
        });
    this.#remoteSynchronizer =
      options.escodeBuiltinRemote &&
      this.#escodeBuiltinSource instanceof NodeESCodeBuiltinProviderConfigSource
        ? new ESCodeBuiltinRemoteSynchronizer({
            source: this.#escodeBuiltinSource,
            ...options.escodeBuiltinRemote,
          })
        : undefined;
    this.#onRemoteRefreshError = options.onESCodeBuiltinRefreshError;
    this.#personalRepository = new NodePersonalProviderConfigRepository({
      filePath: options.personalFilePath,
      onRecovery: options.onPersonalConfigRecovery,
      onPollingError: options.onPersonalConfigPollingError,
      pollingIntervalMs: options.personalPollingIntervalMs,
      ...(options.importLegacy
        ? {
            importLegacy: async () => options.importLegacy!(await this.#escodeBuiltinSource.read()),
          }
        : {}),
    });
    this.configService = new ProviderConfigService({
      escodeBuiltinSource: this.#escodeBuiltinSource,
      personalRepository: this.#personalRepository,
    });
  }

  resolveESCodeBuiltinActiveFilePath(): Promise<string> {
    return this.#escodeBuiltinSource instanceof NodeESCodeBuiltinProviderConfigSource
      ? Promise.resolve(this.#escodeBuiltinSource.activeFilePath)
      : this.#escodeBuiltinSource.resolveActiveFilePath();
  }

  get personalRepository(): import("@escode/provider").PersonalProviderConfigRepository {
    return this.#personalRepository;
  }

  /** Environment 同一周期检查中恢复未对齐依赖，不被下载 TTL 或失败挡住。 */
  onDidCheckESCodeBuiltin(listener: () => Promise<void>): () => void {
    this.#checkListeners.add(listener);
    return () => this.#checkListeners.delete(listener);
  }

  start(): Promise<void> {
    if (this.#disposed) throw new Error("NodeProviderConfigRuntime 已 dispose");
    if (this.#startPromise) return this.#startPromise;
    const startPromise = this.configService.read().then(() => {
      if (this.#disposed) return;
      void this.#checkBackground();
      // Managed Worker 无下载配置也无恢复 owner，不建立周期任务。
      if (
        this.#remoteSynchronizer ||
        this.#escodeBuiltinSource instanceof EndpointScopedESCodeBuiltinSource ||
        this.#checkListeners.size > 0
      ) {
        this.#checkTimer = setInterval(() => {
          void this.#checkBackground();
        }, 60_000);
        this.#checkTimer.unref?.();
      }
    });
    this.#startPromise = startPromise;
    void startPromise.catch(() => {
      if (this.#startPromise === startPromise) this.#startPromise = null;
    });
    return startPromise;
  }

  refreshESCodeBuiltin(options?: { readonly force?: boolean }): Promise<ESCodeBuiltinRefreshResult> {
    if (this.#disposed) return Promise.resolve("disposed");
    if (this.#escodeBuiltinSource instanceof EndpointScopedESCodeBuiltinSource) {
      return this.#escodeBuiltinSource.refresh(options);
    }
    return this.#remoteSynchronizer?.refresh(options) ?? Promise.resolve("skipped");
  }

  #checkBackground(): Promise<void> {
    if (this.#disposed) return Promise.resolve();
    if (this.#checkInFlight) return this.#checkInFlight;
    const check = Promise.allSettled([
      this.refreshESCodeBuiltin(),
      ...[...this.#checkListeners].map((listener) => Promise.resolve().then(listener)),
    ])
      .then((results) => {
        if (this.#disposed) return;
        for (const result of results)
          if (result.status === "rejected") this.#onRemoteRefreshError?.(result.reason);
      })
      .finally(() => {
        if (this.#checkInFlight === check) this.#checkInFlight = null;
      });
    this.#checkInFlight = check;
    return check;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    if (this.#checkTimer) clearInterval(this.#checkTimer);
    this.#checkTimer = null;
    this.#checkListeners.clear();
    this.#remoteSynchronizer?.dispose();
    this.configService.dispose();
    this.#personalRepository.dispose();
    this.#escodeBuiltinSource.dispose();
  }
}

export function createNodeProviderConfigRuntime(
  options: NodeProviderConfigRuntimeOptions,
): NodeProviderConfigRuntime {
  return new NodeProviderConfigRuntime(options);
}
