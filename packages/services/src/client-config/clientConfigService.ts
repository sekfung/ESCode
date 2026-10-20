import {
  buildESCodeEndpointUrls,
  clientConfigReadOptionsSchema,
  normalizeDynamicWorkflowMode,
  parseClientConfigSnapshot,
  resolveDynamicWorkflowClientConfig,
  ESCODE_DYNAMIC_WORKFLOW_MODE_ENV,
  type ApiClient,
  type ClientConfigSnapshot,
  type DynamicWorkflowClientConfig,
} from "@escode/shared";
import { createServiceLogger } from "#src/logger/serviceLogger.js";
import type { IClientConfigService } from "./clientConfig.js";

const CACHE_TTL_MS = 60 * 60 * 1000;
const REQUEST_TIMEOUT_MS = 15_000;

interface RequestContext {
  endpointOrigin: string;
  appVersion: string;
  platform: string;
}

interface CacheEntry {
  snapshot?: ClientConfigSnapshot;
  expiresAt: number;
  pending?: Promise<ClientConfigSnapshot>;
}

/** client/configs 原始信封：只取动态工作流需要的键，其余键原样忽略。 */
interface ClientConfigsEnvelope {
  data?: {
    configs?: {
      dynamicWorkflow?: unknown;
    };
  };
}

/**
 * 窗口级公开配置读取。业务模块只消费自己的字段，不拥有第二份请求缓存。
 * 动态工作流灰度原先由 coding-plan-subscription 通道承载；套餐族删除后收拢到这里，
 * 共用同一个 client/configs 端点与缓存。
 */
export function createClientConfigService(dependencies: {
  apiClient: ApiClient;
  resolveRequestContext: () => RequestContext | Promise<RequestContext>;
  /**
   * 用户在设置里选的动态工作流模式（launch.md「The user's choice」）。只有拥有设置权威的装配
   * （desktop 本地 Host、web/standalone server）注入；缺省时快照只含服务端的「提供」。
   */
  resolveDynamicWorkflowUserMode?: () => Promise<unknown>;
}): IClientConfigService {
  const entries = new Map<string, CacheEntry>();
  const log = createServiceLogger("client-config");

  // 原始信封缓存（与 getSnapshot 同一端点、同一 TTL；解析口径不同，两份缓存共存）。
  let rawEnvelope: ClientConfigsEnvelope | null = null;
  let rawEnvelopeExpiresAt = 0;
  let rawEnvelopePending: Promise<ClientConfigsEnvelope> | null = null;
  async function getRawConfigsEnvelope(forceRefresh: boolean): Promise<ClientConfigsEnvelope> {
    if (!forceRefresh && rawEnvelope && rawEnvelopeExpiresAt > Date.now()) {
      return rawEnvelope;
    }
    if (!rawEnvelopePending) {
      const context = await dependencies.resolveRequestContext();
      const url = new URL(
        "/api/v1/client/configs",
        buildESCodeEndpointUrls(context.endpointOrigin).origin,
      );
      url.searchParams.set("app_version", context.appVersion);
      url.searchParams.set("platform", context.platform);
      rawEnvelopePending = fetchResponseJson(url)
        .then((payload) => {
          rawEnvelope = payload as ClientConfigsEnvelope;
          rawEnvelopeExpiresAt = Date.now() + CACHE_TTL_MS;
          return rawEnvelope;
        })
        .finally(() => {
          rawEnvelopePending = null;
        });
    }
    return rawEnvelopePending;
  }

  async function fetchResponseJson(url: URL): Promise<unknown> {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timedOut = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        controller.abort();
        reject(new Error("Public client config request timed out"));
      }, REQUEST_TIMEOUT_MS);
      timer.unref?.();
    });
    try {
      return await Promise.race([
        (async () => {
          const response = await dependencies.apiClient.request(url, {
            method: "GET",
            credentials: "omit",
            redirect: "error",
            signal: controller.signal,
          });
          if (!response.ok) throw new Error(`Public client config HTTP ${response.status}`);
          // 设置中的 endpoint 可能在两次 await 之间变化；不能把新地址的响应缓存到旧地址。
          if (response.url && response.url !== url.toString()) {
            throw new Error("Public client config request context changed");
          }
          return (await response.json()) as unknown;
        })(),
        timedOut,
      ]);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }

  return {
    async getSnapshot(options = {}) {
      const { forceRefresh } = clientConfigReadOptionsSchema.parse(options);
      const context = await dependencies.resolveRequestContext();
      const url = new URL(
        "/api/v1/client/configs",
        buildESCodeEndpointUrls(context.endpointOrigin).origin,
      );
      url.searchParams.set("app_version", context.appVersion);
      url.searchParams.set("platform", context.platform);
      const key = url.toString();
      const entry = entries.get(key) ?? { expiresAt: 0 };
      entries.set(key, entry);
      if (!forceRefresh && entry.snapshot && entry.expiresAt > Date.now()) {
        return structuredClone(entry.snapshot);
      }
      if (!entry.pending) {
        entry.pending = fetchResponseJson(url)
          .then((payload) => {
            const snapshot = parseClientConfigSnapshot(payload);
            entry.snapshot = snapshot;
            entry.expiresAt = Date.now() + CACHE_TTL_MS;
            return snapshot;
          })
          .finally(() => {
            entry.pending = undefined;
          });
      }
      return structuredClone(await entry.pending);
    },

    async getDynamicWorkflowClientConfig(options = {}) {
      const { forceRefresh } = clientConfigReadOptionsSchema.parse(options);
      const env: Record<string, string | undefined> = process.env;
      const resolveUserMode = dependencies.resolveDynamicWorkflowUserMode;
      const userMode = resolveUserMode ? await resolveUserMode() : undefined;
      // 覆盖合法即短路：判据（normalize）与快照构造（resolve）都留在 shared，这里不复述取值域。
      if (normalizeDynamicWorkflowMode(env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV])) {
        return resolveDynamicWorkflowClientConfig({ remote: undefined, env, userMode });
      }
      try {
        const payload = await getRawConfigsEnvelope(forceRefresh === true);
        return resolveDynamicWorkflowClientConfig({
          remote: payload.data?.configs?.dynamicWorkflow,
          env,
          userMode,
        });
      } catch (error) {
        log.warn(undefined, "动态工作流灰度配置读取失败，按关闭处理", {
          errorMessage: error instanceof Error ? error.message : String(error),
        });
        return resolveDynamicWorkflowClientConfig({ remote: undefined, env, userMode });
      }
    },
  };
}
