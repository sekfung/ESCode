/**
 * remote runtime 发布根与对象前缀的单一口径。
 *
 * 路径段是 CDN 上的真实对象 key（发布侧约定），不是品牌标识：品牌更名曾把它替换成
 * `escode/electron/releases`，会让所有兜底地址整体 404。取值方必须都从这里读：
 *
 * - 运行时兜底：`packages/desktop/src/main/remoteCdn.ts`
 * - 构建期注入：`packages/desktop/tsup.config.ts`（`OSS_PATH_PREFIX` 默认值）
 * - 构建期下载：`packages/desktop/scripts/prepare-bundled-remote-assets.ts`
 *
 * 依据见 docs/specs/escode-rebrand.md 与 docs/specs/offline-remote-assets.md。
 */
export const DEFAULT_OSS_PATH_PREFIX = "zcode/electron/releases";

export const REMOTE_CDN_DOMAINS = Object.freeze({
  domestic: "cdn.codegeex.cn",
  overseas: "cdn-zcode.z.ai",
});

export const REMOTE_CDN_RELEASE_ROOTS = Object.freeze({
  domestic: `https://${REMOTE_CDN_DOMAINS.domestic}/${DEFAULT_OSS_PATH_PREFIX}`,
  overseas: `https://${REMOTE_CDN_DOMAINS.overseas}/${DEFAULT_OSS_PATH_PREFIX}`,
});

function parseCsvEnv(value) {
  return (value ?? "")
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function stripTrailingSlashes(value) {
  return value.replace(/\/+$/, "");
}

/**
 * 构建期下载 remote 制品时的候选发布根（不含版本号）。
 *
 * 优先级与运行时一致：显式覆盖 > CI 注入（CDN_DOMAIN/OSS_PATH_PREFIX）> 官方兜底。
 * 候选按顺序尝试，首个成功者生效。
 */
export function resolveRemoteCdnReleaseBaseCandidates(runtimeEnv = process.env) {
  const override = runtimeEnv.ESCODE_REMOTE_ASSET_CDN_BASE_URL?.trim();
  if (override) {
    return [stripTrailingSlashes(override)];
  }

  const domains = parseCsvEnv(runtimeEnv.CDN_DOMAIN);
  if (domains.length > 0) {
    const pathPrefixes = parseCsvEnv(runtimeEnv.OSS_PATH_PREFIX || DEFAULT_OSS_PATH_PREFIX);
    if (pathPrefixes.length !== 1 && pathPrefixes.length !== domains.length) {
      throw new Error(
        `OSS_PATH_PREFIX must contain one value or match CDN_DOMAIN count (${domains.length}), ` +
          `got ${pathPrefixes.length}`,
      );
    }
    return domains.map((domain, index) =>
      stripTrailingSlashes(
        `https://${domain}/${pathPrefixes.length === 1 ? pathPrefixes[0] : pathPrefixes[index]}`,
      ),
    );
  }

  return [REMOTE_CDN_RELEASE_ROOTS.overseas, REMOTE_CDN_RELEASE_ROOTS.domestic];
}
