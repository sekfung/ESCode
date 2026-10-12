/** 官方 zcode 发布路径前缀；CDN 对象 key，属发布侧约定，不随品牌更名改动。 */
export const DEFAULT_OSS_PATH_PREFIX: string;

/** 官方 CDN 域名。 */
export const REMOTE_CDN_DOMAINS: Readonly<{ domestic: string; overseas: string }>;

/** 官方 remote 发布根（域名 + 默认路径前缀，不含版本号）。 */
export const REMOTE_CDN_RELEASE_ROOTS: Readonly<{ domestic: string; overseas: string }>;

/**
 * 构建期下载 remote 制品时的候选发布根（不含版本号）。
 * 优先级：`ESCODE_REMOTE_ASSET_CDN_BASE_URL` > `CDN_DOMAIN`/`OSS_PATH_PREFIX` > 官方兜底。
 */
export function resolveRemoteCdnReleaseBaseCandidates(
  runtimeEnv?: Record<string, string | undefined>,
): string[];
