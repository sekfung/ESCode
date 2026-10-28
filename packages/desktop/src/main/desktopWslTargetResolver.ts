import type { RemoteTarget } from "@escode/shared";

type WslTarget = Extract<RemoteTarget, { kind: "wsl" }>;

interface WslIdentityBackend {
  resolveIdentity(): Promise<{ distro: string; user: string }>;
  dispose(): void;
}

interface CanonicalWslTargetResolverOptions {
  createBackend?: (target: WslTarget) => WslIdentityBackend | Promise<WslIdentityBackend>;
  ttlMs?: number;
  now?: () => number;
}

interface CachedResolution {
  expiresAt: number;
  promise: Promise<WslTarget>;
}

const DEFAULT_RESOLUTION_TTL_MS = 5_000;
const DEFAULT_IDENTITY_SEGMENT = "<default>";

function buildProvisionalResolutionKey(target: WslTarget): string {
  const distro = target.distro?.trim().toLowerCase() || DEFAULT_IDENTITY_SEGMENT;
  const user = target.user?.trim() || DEFAULT_IDENTITY_SEGMENT;
  return `${distro}\0${user}`;
}

export function createCanonicalWslTargetResolver(
  options: CanonicalWslTargetResolverOptions = {},
): (target: WslTarget) => Promise<WslTarget> {
  const createBackend =
    options.createBackend ??
    (async (target: WslTarget) => {
      // WSLBackend 连同 remote 子系统的依赖只在真正解析 WSL 目标时加载，避免进入 main 首屏图。
      const { WSLBackend } = await import("@escode/server/remote/wsl-backend.js");
      return new WSLBackend(target);
    });
  const ttlMs = Math.max(1, Math.floor(options.ttlMs ?? DEFAULT_RESOLUTION_TTL_MS));
  const now = options.now ?? Date.now;
  const cache = new Map<string, CachedResolution>();

  return (target) => {
    const key = buildProvisionalResolutionKey(target);
    const cached = cache.get(key);
    if (cached && cached.expiresAt > now()) {
      return cached.promise;
    }

    const entry: CachedResolution = {
      expiresAt: now() + ttlMs,
      promise: Promise.resolve().then(async () => {
        const backend = await createBackend(target);
        try {
          const identity = await backend.resolveIdentity();
          return {
            kind: "wsl" as const,
            distro: identity.distro,
            user: identity.user,
          };
        } finally {
          backend.dispose();
        }
      }),
    };
    cache.set(key, entry);
    void entry.promise.catch(() => {
      // 临时 discovery 失败若留在 cache，会让用户在 TTL 内无法通过重试恢复连接。
      if (cache.get(key) === entry) {
        cache.delete(key);
      }
    });
    return entry.promise;
  };
}

export const resolveCanonicalWslTarget = createCanonicalWslTargetResolver();
