/**
 * remote runtime 发布根与对象前缀的单一口径。
 *
 * 路径段是 CDN 上的真实对象 key（发布侧约定），不是品牌标识：品牌更名曾把它替换成
 * `escode/electron/releases`，会让所有兜底地址整体 404。取值方必须都从这里读：
 *
 * - 运行时兜底：`packages/desktop/src/main/remoteCdn.ts`
 * - 构建期注入：`packages/desktop/tsup.config.ts`（`OSS_PATH_PREFIX` 默认值）
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
