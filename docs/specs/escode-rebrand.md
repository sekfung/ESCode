# 产品更名：ZCode → ESCode

2026-10-11。本仓库源自 Z.AI Co., Ltd 以 Apache License 2.0 开源的 ZCode。本次把产品更名为 ESCode，在不违反 Apache-2.0 的前提下尽可能去除 ZCode 标识。

## Apache-2.0 约束与处理

| 条款  | 要求                                   | 处理                                                                                             |
| ----- | -------------------------------------- | ------------------------------------------------------------------------------------------------ |
| §4(a) | 随分发附带许可证副本                   | 根目录 `LICENSE` 原样保留（含附录中的 `Copyright 2026 Z.AI Co., Ltd`）                           |
| §4(b) | 修改过的文件带有显著的修改声明         | 新 `NOTICE.md` 声明本仓库为 ZCode 的衍生作品、全部文件均可能经 ESCode 贡献者修改，并说明更名范围 |
| §4(c) | 保留源码中的版权、专利、商标与署名声明 | 不删除任何既有版权 / 署名行；第三方目录、`THIRD-PARTY-NOTICES.md` 中的第三方声明不改动语义       |
| §4(d) | 附带 NOTICE 中的署名声明               | 上游 `NOTICE.md` 原文移至 `third-party/upstream-zcode/NOTICE.md` 逐字保留，新 `NOTICE.md` 指向它 |
| §6    | 许可不授予商标权                       | 产品名、图标、字标、协议名与标识全部改为 ESCode，避免使用 ZCode 商标                             |

## 更名范围

批量替换（内容与路径）：`ZCODE→ESCODE`、`ZCode→ESCode`、`Zcode→Escode`、`zCode→esCode`、`zcode→escode`，并单独清理带空格的旧写法 `Z Code`（如退出确认文案、`X-Title` 客户端标识）。覆盖产品名与界面文案、包名（`@zcode/*` → `@escode/*`）、目录（`apps/zcode-cli` → `apps/escode-cli` 等）、Rust crate 与二进制名、环境变量（`ZCODE_*` → `ESCODE_*`）、数据目录（`~/.zcode` → `~/.escode`）、项目配置目录（`.zcode/` → `.escode/`）、插件清单目录（`.zcode-plugin` → `.escode-plugin`，旧目录仍可读）、协议名（ZCode Protocol → ESCode Protocol）、深链 scheme（只用 `escode://`）。

图标：桌面应用图标（png / ico / icns，含安装器与 Windows 变体）、dmg 安装背景、应用内字标与加载 / 关于页 / 空状态的内联 SVG 全部替换为 ESCode 斜体「E」字标（中横强调色 `#22D3EE`）与 ESCODE 字标。

## 刻意保留（外部线上契约）

改名会让对应功能直接失效，以下保留并在此登记：

| 保留项                                                                                         | 原因                                                                                                                                                                                                |
| ---------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 主机为 `zcode.z.ai`、`cdn-zcode.z.ai`、`zcode.ai` 的 URL（含正则里的转义写法），以及这些域名下的 `/zcode/...` 资源路径 | Z.ai 提供的账号、Coding Plan、远控中继、事件上报、插件 CDN 等服务端点；路径与远端 CDN 上实际发布位置一致，改名会 404 |
| `zcode-plan`（如 `/api/v1/zcode-plan/...`）                                                    | 上述服务的接口路径与 provider 标识                                                                                                                                                                  |
| `x-zcode-*` 请求头名                                                                           | 发往上述服务的请求头，服务端按名读取                                                                                                                                                                |
| `zcode-websearch-client`                                                                       | 联网搜索服务的客户端标识                                                                                                                                                                            |
| 官方插件市场 id `zcode-plugins-official`                                                       | 必须与 Z.ai 插件 CDN `marketplace.json` 的 `name` 一致，否则官方市场刷新被保留名校验拒绝；内置插件 id 因此仍为 `<name>@zcode-plugins-official`。UI 展示名不按 slug 推导，使用固定展示名 `ESCode Plugins Official`（`ESCODE_OFFICIAL_PLUGIN_MARKETPLACE_DISPLAY_NAME`） |
| 旧插件清单目录 `.zcode-plugin/`                                                                | CDN 与既有第三方插件仍以它发布：通用发现 / 安装 / zip / 同步按 `.escode-plugin` → `.zcode-plugin` → `.claude-plugin` → `.codex-plugin` 取第一个存在的 manifest；本仓库自带插件只用 `.escode-plugin` |
| `LICENSE`、`third-party/upstream-zcode/`、上游许可与第三方原文、文档镜像（`docs/electron/docs/**`、`docs/knip/**`） | 许可与上游 / 第三方原文。口径收紧（2026-10-12）：`docs/electron/*.md` 是本仓库自有文档、`third-party/{README.md,SPEC.md}` 是自有维护文档，均需随实现同步，不属镜像豁免 |
| README 与 `NOTICE.md` 中的上游署名（ZCode、Z.AI Co., Ltd）                                     | Apache-2.0 §4(b)/(d) 要求的衍生作品与修改声明、上游署名，不能删除                                                                                                                                   |

## 不保留的 Z.ai 专属契约（2026-10-11 用户确认）

用户确认不需要 Z.ai 账号登录与 Coding Plan 套餐，以下按 ESCode 改名、不做兼容：

- 深链 scheme 只有 `escode://`（OAuth 回调 `escode://oauth/callback`、支付回跳、分享导入、打开工作区）；Z.ai / BigModel 登记的是 `zcode://` 回调与 `appId=zcode`，因此官方登录、套餐购买回跳将不可用。
- 与 Z.ai 托管页面共享的桥接与帧字段（Coding Plan 购买页的 `window.escodeBridge`、手机远控页的 `escode_type`、协议名 `ESCode Protocol` 等）、MCP `_meta["escode/…"]` 扩展键与遥测事件名同样改名；依赖它们的 Z.ai 托管页面与按旧键声明的第三方插件扩展不再生效。

服务端点主机名（`zcode.z.ai` 等）不是品牌标识而是真实地址，仍保留，模型网关、联网搜索等非登录功能照常调用。

`pnpm-lock.yaml`、`Cargo.lock` 与生成资产在更名后重新生成而非手改。

### CDN 发布路径与远程制品命名（已按官方仓库收敛）

更名时 `zcode` 被一并替换成 `escode`，把 CDN 对象前缀和远程制品的文件名也改错了。
两者都是发布侧的真实约定，不是品牌标识，已恢复为官方值：

- 发布路径前缀统一为 `zcode/electron/releases`。单一口径在
  `scripts/remote-cdn-roots.mjs`（`DEFAULT_OSS_PATH_PREFIX` 与 `REMOTE_CDN_RELEASE_ROOTS`）：
  运行时 `remoteCdn.ts`、构建期注入 `tsup.config.ts`、构建期下载
  `prepare-bundled-remote-assets.ts` 都从它取值。此前 `remoteCdn.ts` 的 `domestic` 兜底是
  `https://cdn.codegeex.cn/escode/electron/releases`（实测 404），已修正。
- 远程制品名统一为上游名：server bundle `zcode-server.cjs`、agent bundle `zcode.cjs`
  （`escode-server.cjs` / `escode.cjs` 在远程发布链路上已全面替换）。
  CLI 自身构建产物 `apps/escode-cli/packages/cli/dist/escode.cjs` 不属远程制品，保持原名。

依据（2026-10-12 核对）：

- 官方仓库 `zai-org/ZCode`：`remoteCdn.ts` 国内根为 `https://cdn.codegeex.cn/zcode/electron/releases`；
  `tsup.config.ts` 的 `OSS_PATH_PREFIX` 默认值为 `zcode/electron/releases`；`remoteCdn.test.ts` 断言同值。
- 线上正式包 `app.asar` 注入的候选列表是 `https://cdn-zcode.z.ai/zcode/electron/releases`，
  且不含任何 `escode/electron/releases`。
- CDN 实测：`.../escode/electron/releases/3.14.5/manifest-linux-x64.json` 返回 404，
  `.../zcode/electron/releases/3.14.5/manifest-linux-x64.json` 正常；
  该版本 server-bundle / glm 归档解出的是 `zcode-server.cjs` 与 `zcode.cjs`。

`*.example.com` 之类的占位域名未改（其路径不代表真实发布位置）。

## 更名工具（一次性）

批量替换与图标生成在仓库外 `C:\Users\sekfung\Projects\escode-rebrand-tools\`：`rebrand.py`（批量替换）、`finish_paths.py`（嵌套路径改名）、`logos.py`（内联标识替换）、`legacy_manifest.py`（`.zcode-plugin` 回退）、`deeplink_revert.py`（撤销 `zcode://` 别名）均已执行，**不要再跑**；`icons/` 是图标生成器（`node build.mjs out`，`out/` 为产物），可在需要时重新生成图标——安装素材 `icon_installer.*` 为浅底变体，与应用图标成对且可区分。

批量文本替换要单独检查含字面 NUL 分隔符的文件（如 `packages/ui/src/hooks/useWorkflowRunNodeResult.ts`），通用脚本会把它当二进制跳过。

## 验收

- 残留扫描：除上表保留项外，受控文件中不再出现 `zcode`（大小写不敏感）。`docs/electron/*.md`、`third-party/{README.md,SPEC.md}` 是本仓库自有文档（非镜像豁免），2026-10-12 已同步为 ESCode 现状。
- `pnpm typecheck`、`pnpm lint`、CLI 全包 typecheck、Rust `cargo test --workspace` 与漂移检查通过。
- 桌面图标与 dmg 背景为 ESCode 标识；安装素材 `icon_installer.*` 为浅底变体（2026-10-12 重新生成——更名时曾与应用图标逐字节同源，与 electron-builder 注释的「专用素材」不符）。
- 更名操作手册 `docs/handoff/2026-10-11-escode-rebrand-handoff.md` 已删除，可复用内容（本机 GNU 工具链、App 差分准备与本机坑）并入 `docs/specs/rust-windows-parity.md`。
