import {
  MutableAccountProviderConfigSource,
  createFailClosedAccountProviderConfigSnapshot,
  parseAccountProviderConfigMap,
  type AccountProviderConfigSnapshot,
  type AccountProviderStates,
} from "@escode/provider";
import {
  isBuiltinModelProviderId,
  resolveRuntimeESCodeEndpointOrigin,
  ESCODE_VERSION,
} from "@escode/shared";
import { dirname, join } from "node:path";
import {
  NodeModelSelectionConfigRepository,
  NodeProviderRegistryRuntime,
  resolveNodeProviderRuntimePaths,
  downloadESCodeBuiltinRelease,
  resolveESCodeBuiltinClientPlatform,
  ESCODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV,
  type ESCodeBuiltinRefreshEvent,
} from "@escode/provider-node";
import { readLegacyCliPersonalProviderConfig } from "./legacy-cli-personal-provider-config-importer.js";

export interface ProcessProviderRegistryRuntimeOptions {
  /** Standalone Prompt CLI / TUI 自己拥有旧配置的一次性导入与 Built-in 刷新。 */
  readonly standalone?: {
    readonly legacyCliUserConfigFilePath?: string;
    readonly request?: typeof fetch;
    readonly onBuiltinRefreshError?: (error: unknown) => void;
    readonly onBuiltinRefreshResult?: (event: ESCodeBuiltinRefreshEvent) => void;
  };
}

export async function startProcessProviderRegistryRuntime(
  env: Readonly<Record<string, string | undefined>>,
  options: ProcessProviderRegistryRuntimeOptions = {},
) {
  const paths = resolveNodeProviderRuntimePaths(env);
  if (!paths) {
    throw new Error("缺少进程 Provider Registry 的 ESCode Built-in / Personal Config 路径");
  }

  const accountSource = new MutableAccountProviderConfigSource();
  const bundledFile = options.standalone
    ? env[ESCODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV]?.trim()
    : undefined;
  const runtime = new NodeProviderRegistryRuntime({
    ...paths,
    ...(bundledFile
      ? {
          escodeBuiltinFilePath: bundledFile,
          escodeBuiltinActiveFilePath: paths.escodeBuiltinFilePath,
          escodeBuiltinRemote: {
            controlFilePath: join(
              dirname(paths.escodeBuiltinFilePath),
              "escode-builtin-refresh.json",
            ),
            resolveEndpointKey: () => resolveRuntimeESCodeEndpointOrigin(env),
            fetchRelease: (endpointOrigin, signal) =>
              downloadESCodeBuiltinRelease({
                endpointOrigin,
                signal,
                appVersion: ESCODE_VERSION,
                platform: resolveESCodeBuiltinClientPlatform(),
                request: options.standalone?.request ?? globalThis.fetch,
              }),
            onRefreshResult: options.standalone?.onBuiltinRefreshResult,
          },
        }
      : {}),
    onESCodeBuiltinRefreshError: options.standalone?.onBuiltinRefreshError,
    accountSource,
    ...(options.standalone
      ? {
          importLegacy: () =>
            readLegacyCliPersonalProviderConfig({
              ...(options.standalone?.legacyCliUserConfigFilePath
                ? { filePath: options.standalone.legacyCliUserConfigFilePath }
                : {}),
            }),
        }
      : {}),
  });
  // 下线 Standalone 账号服务后，Built-in 刷新不再有账号解析方同步 revision。
  // Registry 见 account.basedOnESCodeBuiltinRevision 落后会继续服务旧快照，
  // 导致远端下载成功后 Registry 永远停在旧 revision。这里在 Built-in 源变化后
  // 用 fail-closed 的空账号覆盖对齐 revision，等待 Host 重新下发账号事实。
  const disposeAccountRevisionReconciliation = runtime.configService.onDidChange((reason) => {
    if (!reason.startsWith("escodeBuiltin:")) return;
    void (async () => {
      const config = await runtime.configService.read();
      const account = await accountSource.read();
      if (account.basedOnESCodeBuiltinRevision === config.escodeBuiltinRevision) return;
      accountSource.replace(
        createFailClosedAccountProviderConfigSnapshot(config),
        "escode-builtin-changed",
      );
    })().catch((error: unknown) => options.standalone?.onBuiltinRefreshError?.(error));
  });
  try {
    await runtime.start();
    const snapshot = runtime.registryService.getSnapshot()!;
    const modelSelectionConfigRepository = new NodeModelSelectionConfigRepository({
      personalRepository: runtime.personalRepository,
    });
    try {
      const configuredDefaultModelSelection = await modelSelectionConfigRepository.read();
      return Object.freeze({
        accountSource,
        async syncAccountProviderConfig(next: AccountProviderConfigSnapshot): Promise<boolean> {
          const changed = accountSource.replace(next, "host-account-config");
          // Source 去重只证明收过，不证明上次刷新成功。重交时仍刷新；配套配置未到
          // 则由 Registry 保留完整旧快照，不能把接收确认冒充应用确认。
          await runtime.registryService.refresh("host-account-config");
          return changed;
        },
        dispose() {
          disposeAccountRevisionReconciliation();
          modelSelectionConfigRepository.dispose();
          runtime.dispose();
        },
        runtime,
        snapshot,
        modelSelectionConfigRepository,
        configuredDefaultModelSelection,
      });
    } catch (error) {
      disposeAccountRevisionReconciliation();
      modelSelectionConfigRepository.dispose();
      throw error;
    }
  } catch (error) {
    disposeAccountRevisionReconciliation();
    runtime.dispose();
    throw error;
  }
}

/** 把协议信封解析为进程 Registry 使用的第三层 Account Config Overlay。 */
export function parseProcessAccountProviderConfigSnapshot(input: {
  readonly revision: string;
  readonly basedOnESCodeBuiltinRevision: string;
  readonly providers: unknown;
  readonly states?: AccountProviderStates;
}): AccountProviderConfigSnapshot {
  const revision = input.revision.trim();
  if (!revision) throw new Error("Account Config revision 不能为空");
  const basedOnESCodeBuiltinRevision = input.basedOnESCodeBuiltinRevision.trim();
  if (!basedOnESCodeBuiltinRevision) {
    throw new Error("Account Config Built-in revision 不能为空");
  }
  const providers = parseAccountProviderConfigMap(input.providers);
  for (const [providerId, provider] of providers.entries()) {
    // 仅约束托管 Worker 的普通账号信封；独立 CLI、API 和闲时不需要 current。
    if (
      isBuiltinModelProviderId(providerId) &&
      provider.access?.type === "zhipu-account" &&
      provider.access.entitled &&
      typeof input.states?.[providerId]?.current !== "boolean"
    ) {
      throw new Error(`Account State 缺少 current: ${providerId}`);
    }
  }
  return Object.freeze({
    revision,
    basedOnESCodeBuiltinRevision,
    providers,
    // 与 Overlay 属于同一快照；不能只更新 revision 却丢掉当前连接事实。
    ...(input.states ? { states: input.states } : {}),
  });
}
