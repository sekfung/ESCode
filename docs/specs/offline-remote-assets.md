# 离线远程连接：安装包内置远程运行时资源

## 1. 状态与范围

- 状态：已实现
- 目标：内网 / 无外网机器上的 ESCode Desktop 也能使用 SSH / WSL / Docker 远程工作区，首次连接不再需要从 CDN 下载目标平台运行时
- 本文定位：内置远程资源的平台映射、运行时判据、打包流程与已知限制

不在本规格范围：

- `remote-download`（远端自行 curl CDN）模式的离线支持
- 未内置的远程平台（见 §4）的离线支持
- `server` 类型远程目标：它连接的是已经运行的 ESCode server，本就不部署任何资源

## 2. 背景

远程工作区首次连接要把目标平台的运行时装进远端机器。生产安装包过去刻意不携带这些资源，
统一按 `manifest-<platform-arch>.json` 从 CDN 逐组件下载到本地 `remote-assets-cache`，
再由 Desktop 上传（`local-download-upload`）或交给远端自行下载（`remote-download`）：

| CDN 根 | 用途 |
| --- | --- |
| `https://cdn.codegeex.cn/zcode/electron/releases` | 国内 |
| `https://cdn-zcode.z.ai/zcode/electron/releases` | 海外 |

代价是无外网环境无法建立远程连接，即使远端机器本身可达。安装包携带一份本地 release 即可消除这次下载。

## 3. 核心决策

| 决策项 | 结论 |
| --- | --- |
| 资源来源 | 复用已有的「本地 release 目录」源（`mockCdnDir`），不新增第二套本地物化路径 |
| 唯一所有者 | 资产源的选择仍由 `resolveRemoteAssetDirs()` 独占决定（desktop main），Host 与 deploy 只消费结果 |
| 平台映射 | 按构建目标的 **arch** 取同架构的 Linux release（§4） |
| 产物形态 | 直接进入标准安装包，不额外提供精简包 |
| 平台无关资源 | `server/` 一份即可；`glm/`、`node-pty/`、`node/`、`tools/` 按平台取一份 |
| 缺失行为 | 内置资源不完整或平台未内置时，按组件回退 CDN/cache，行为与改造前一致 |

选「同架构 Linux」而不是「同 OS 同架构」的原因：远程后端只支持 POSIX shell（Windows 原生远程主机被
`assertSupportedRemoteEnvironment` 拒绝），而 Windows 桌面的远程目标（WSL、SSH、Docker）实际都是 Linux；
macOS 桌面的主流远程目标同样是 Linux 服务器。

## 4. 平台映射

| 构建目标（`ESCODE_TARGET_OS`/`ESCODE_TARGET_ARCH`） | 内置远程平台 |
| --- | --- |
| `win32-x64` / `linux-x64` / `darwin-x64` | `linux-x64` |
| `win32-arm64` / `linux-arm64` / `darwin-arm64` | `linux-arm64` |

其余远程平台（`darwin-x64`、`darwin-arm64`、以及另一架构的 Linux）继续走 CDN。

## 5. 资源布局

安装包内：

```text
resources/remote-assets/
└── releases/<ESCODE_VERSION>/
    ├── manifest-linux-<arch>.json
    ├── server/
    │   └── zcode-server.cjs
    ├── node/linux-<arch>/node
    ├── node-pty/linux-<arch>/pty.node
    ├── glm/linux-<arch>/zcode.cjs
    ├── glm/linux-<arch>/escode-cli-rust   # musl 静态 Rust runtime，远程默认 runtime（rust-packaging.md「远程环境使用 Rust」）
    └── tools/linux-<arch>/{bfs,rg,ugrep}/
```

版本目录名必须与运行时 `ESCODE_VERSION` 逐字一致：主进程按
`<resources>/remote-assets/releases/<ESCODE_VERSION>` 是否存在来决定是否暴露本地源，名字对不上会
静默退回 CDN 下载。

## 6. 运行时行为

```text
resolveRemoteAssetDirs()
  ├─ 打包态且 <resources>/remote-assets/releases/<ESCODE_VERSION> 存在
  │    └─ mockCdnDir = <resources>/remote-assets         ← 新增
  └─ 否则保持原状（CDN + remote-assets-cache）

deploy（local-download-upload）
  ├─ getManifestRefForComponents → 本地 manifest 完整 → 本地 release 目录
  ├─ resolveReleaseDir → LocalUploadAssetInstaller 直接打包上传
  └─ 本地路径缺失且配置了 CDN → 回退 CDN/cache（与改造前一致）
```

本地源只作为 `mockCdnDir` 暴露给 Host；`remote-download` 模式仍只使用 CDN manifest，因此该模式在
无外网时不可用（见 §8）。

## 7. 打包流程

```text
pnpm build
  └─ prepare:runtime-assets
       ├─ prepare:remote-assets         # 从本仓库源码构建远程 release 到 mock-cdn/releases/<version>/
       │                                #   （server-bundle、glm、官方插件、node-pty、tools；node 二进制取 Node dist）
       │                                #   glm 另含 escode-cli-rust：ESCODE_REMOTE_RUST_BINARIES_DIR 提供或本机 cargo 构建
       │                                # ESCODE_REMOTE_ASSET_PLATFORMS 限定平台，缺省 4 个远程平台
       └─ prepare:bundled-remote-assets # 取 mock-cdn 中目标架构的 linux release：
                                        #   manifest → 组件归档 → sha256 校验 → 解压到 mount
                                        # 组装 bundled-remote-assets/releases/<version>/
                                        # 关键条目缺失或校验失败直接失败
electron-builder
  └─ extraResources: bundled-remote-assets → resources/remote-assets
       └─ afterPack: assertPackagedBundledRemoteAssets 复核产物
```

### 制品来源：本仓库源码构建（2026-10-12，用户决定）

此前内置资源在构建期从 Z.ai CDN 下载**上游 ZCode** 已发布的制品。更名后本地侧契约已改为 escode，
与上游制品不再一致，远程连接出现两类必然故障：

- server bundle 能力标记 `__escode_rpc_nested_uint8array_v1` 在上游 `zcode-server.cjs` 中是
  `__zcode_rpc_nested_uint8array_v1`：每次连接都判定缺标记并重传 server；该标记同时是 RPC 嵌套
  `Uint8Array` 的线上编码键，本地与远端编解码不一致。
- 官方插件必需路径 `<package>/.escode-plugin/plugin.json`（含 `escode-cua-plugin`、`escode-guide-plugin`）
  在上游 glm 组件中是 `.zcode-plugin` 与 `zcode-*` 包名：本地 release 被判不完整，glm 回退 CDN。

因此内置资源改为由 `prepare:remote-assets`（`scripts/prepare-prebuilds.mjs`）从本仓库源码构建，
`prepare:bundled-remote-assets` 只从本地 `mock-cdn` 取目标平台的 manifest 与组件归档，不再访问网络。
本地与远端运行同一份源码，契约天然一致；manifest 的 `appVersion` 即仓库版本，不再要求与上游发布对齐。

- `ESCODE_REMOTE_ASSET_PLATFORMS=linux-x64,...`：`prepare:remote-assets` 只构建列出的远程平台；
  发布 job 按目标架构只构建对应的 linux 平台。
- `ESCODE_SKIP_REMOTE_ASSETS=1`：跳过 mock-cdn 准备，并随之跳过内置资源（本地 bootstrap、`--dir` 验收沿用）。
- `ESCODE_SKIP_BUNDLED_REMOTE_ASSETS=1`：只跳过内置资源，安装包回到 CDN 下载语义。
- 两种跳过都会清掉构建工作区里残留的 `bundled-remote-assets/`：electron-builder 只要该目录存在就会打包，
  残留的旧产物（如曾下载的上游制品）会让「跳过」名不副实。
- `prepare:bundled-remote-assets` 单独运行时找不到本地 manifest 会直接失败，提示先运行 `prepare:remote-assets`。

制品只存在于构建工作区（`mock-cdn/`、`bundled-remote-assets/` 已 gitignore），不需要入库。
本地源不完整即构建失败，避免产出缺离线能力的包。

运行时 CDN 回退（未内置平台、`remote-download` 模式）仍指向上游 CDN，取到的是契约不一致的上游制品；
本仓库目前没有自有 CDN 发布，这些路径的远程连接不保证可用（§8 已知限制 5）。

## 8. 体积与已知限制

实测单平台（`linux-x64`）解压后约 **163 MiB**，其中 Node 运行时二进制约 121 MiB 占主导：

| 组件 | 体积 |
| --- | --- |
| node-runtime | 116 MB |
| glm（Agent bundle） | 26 MB |
| server-bundle | 12 MB |
| ripgrep / ugrep / bfs / node-pty | 5.6 / 4.1 / 1.0 / 0.08 MB |

`win-unpacked` 基线 652 MiB、安装包 142.8 MiB（审计上限 500 MiB）；内置单平台后安装包预计增长几十 MiB，
仍在现有 `audit-bundle-size.mjs` 上限内。

已知限制：

1. `remote-download` 模式仍需要 CDN，无外网时请使用默认的 `local-download-upload`。
2. 未内置的远程平台需要 CDN；内置平台本身不完整时同样回退 CDN。
3. 同一 App 版本内重发组件（例如热修 `glm` / `server-bundle`）不会被内置资源感知：
   内置 manifest 固定为打包时的快照，用户要到下一个 App 版本才会拿到新制品。
   这与「用 CDN manifest 做 SHA 跳过判断」的语义差异，是离线能力换来的取舍；
   紧急热修需要随新版安装包发布。
4. 旧版安装包连接由新版 App 部署过的远端时，仍按 App 版本差异触发强制刷新并部署本版本的资源，
   这一点与改造前一致（CDN 路径同样是取当前 App 版本的 manifest）。
5. 运行时 CDN 回退指向上游 Z.ai CDN，制品与本仓库契约不一致（见 §7「制品来源」）；未内置的远程平台
   （macOS 远端、另一架构的 Linux）与 `remote-download` 模式在本仓库建立自有发布前不保证可用。
   Preview 包（`ESCODE_ENV=test`）的 CDN 根是测试占位地址，回退必然失败。

## 9. 验收

- 单测 `packages/desktop/test/desktopRuntimeEnv.test.ts`：打包态内置当前版本资源时暴露本地源；
  版本目录缺失时不暴露、继续走 CDN。
- 构建期强校验：`prepare-bundled-remote-assets` 在关键条目缺失时失败，避免出厂一个不完整的本地源。
- 产物校验：`afterPack` 断言 `resources/remote-assets` 关键条目存在，防止 extraResources 静默失效。

## 10. 相关文件

- `packages/desktop/src/main/desktopRuntimeEnv.ts`（`resolveRemoteAssetDirs`）
- `packages/desktop/scripts/prepare-bundled-remote-assets.ts`
- `packages/desktop/scripts/prepare-runtime-assets.mjs`
- `packages/desktop/electron-builder.config.js`（`extraResources`、`assertPackagedBundledRemoteAssets`）
- `scripts/prepare-prebuilds.mjs`（`ESCODE_REMOTE_ASSET_PLATFORMS`）
- `packages/server/src/remote/deploy.ts`、`remoteAssetLiveIdentity.ts`、`remoteAssetDeployDecision.ts`（本地源消费方）
- `.github/workflows/release.yml`（桌面构建 job 的远程平台与资源准备）
