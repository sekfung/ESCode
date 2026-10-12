# Release 安装包：正式版产物规范

2026-10-12，用户要求：release 出来的安装包就是正式版，不带 sourcemap、符号表以及名字带 TEST 等调试信息。

## 现场问题

`release.yml` 没有设置 `ESCODE_ENV`。构建脚本按 fail-safe 默认把未知环境当作 `test`，于是 release 产出的是：

- `ESCode Preview` 身份（`dev.escode.app.preview`），可与正式版并排安装；
- 文件名带 `_TEST` 后缀（`resolveDesktopArtifactSuffix`）；
- 内置 provider 配置为 `config/provider/escode-builtin.test.json`，桌面与远程 server bundle 连接测试后端。

## 规则

| 项           | 规则                                                                                     | 所有者                                                                  |
| ------------ | ---------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| 后端环境     | release 工作流级 `ESCODE_ENV: production`；不得设置 `ESCODE_PREVIEW_IDENTITY=1`          | `.github/workflows/release.yml`                                         |
| 身份与文件名 | 正式身份 `ESCode` / `dev.escode.app`，安装包文件名不含 `_TEST` 与 `Preview`              | `desktop-product-identity.mjs`（既有）                                  |
| JS sourcemap | 生产构建不生成或不暴露 sourcemap；`afterPack` 删除 `.map` 并剥离 `sourceMappingURL` 注释 | tsup / vite / agent CLI 构建与 `packaged-sourcemap-cleanup.mjs`（既有） |
| Rust 符号表  | `[profile.release] strip = true`，不开启 `debug`                                         | `apps/escode-cli-rust/Cargo.toml`（既有）                               |

既有机制本身正确，缺口在于 release 没有进入生产环境、且没有产物级断言。

## 发布门禁

`packages/desktop/scripts/assert-release-artifacts.mjs` 在 release 的 `desktop` job 打包后运行（只读，不修改产物），任一项不满足即失败：

1. `ESCODE_ENV` 为 `production`，且未设置 `ESCODE_PREVIEW_IDENTITY=1`；
2. `dist/` 下存在安装包，且文件名不含 `_TEST` / `Preview`；
3. electron-builder 输出的每个已打包应用（`resources/app.asar` 所在目录）：`app.asar` 内与 `resources/` 下不得有 `.map`
   文件，脚本（`.js/.cjs/.mjs/.css`）不得带行首 `sourceMappingURL` 注释（判据与 `packaged-sourcemap-cleanup.mjs` 一致）。

远程资源中的组件归档（`.tar.gz`）不在扫描范围：其源产物（`zcode-server.cjs` esbuild、`zcode.cjs` desktop-agent 模式）构建期
即不生成 sourcemap，Rust 二进制同样走 release profile。

## 验收

- 单测 `packages/desktop/test/assertReleaseArtifacts.test.ts`：文件名判据、sourcemap 判据、Cargo release profile 必须 strip、
  release 工作流必须为生产环境并运行门禁、对真实 asar 组成的 electron-builder 输出目录拦截不合规产物并放行干净产物；
  归档内路径按 `/` 报告，不随打包平台（Windows 的 `\`）变化。
- 未在本机验证：完整的 electron-builder 打包（本机无法下载 Electron 二进制），由 release 工作流的门禁步骤在 CI 上验证。
