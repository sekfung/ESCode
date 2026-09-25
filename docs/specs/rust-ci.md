# Rust runtime 的 GitHub CI 与发布（跨平台验收）

2026-09-25，用户要求：增加 GitHub CI，支持 Windows、macOS、Linux 的构建发布；范围确认为「Rust runtime + 测试 + 桌面安装包」，
macOS 暂不签名。仓库此前没有任何 CI 配置（无 `.github/`、无 GitLab CI）。

## 工作流

### `.github/workflows/rust-runtime-ci.yml`（push 到 main / `feat/**`、PR、手动）

| job               | runner                                        | 内容                                                                                               |
| ----------------- | --------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| checks            | ubuntu-latest                                 | `pnpm check:zcode-cli-rust`（边界、fmt、clippy）、`typecheck`、`lint`、`architecture:check`        |
| test (matrix × 3) | ubuntu-latest / windows-latest / macos-latest | `pnpm test:zcode-cli-rust`：生成资产 `--check`、测试 tsconfig 类型检查、`cargo test`、App 集成套件 |

- Windows runner 默认 MSVC 工具链，覆盖此前本机只能用 GNU 目标得出源码级结论的缺口；macOS runner 为 arm64。
- `ZCODE_TEST_SERIAL=1`：cargo 测试 `--test-threads=1`、App 套件 `--test-concurrency=1`（每个用例起 runtime + 本地模型服务，并发会被接收超时误伤）。
- `ELECTRON_SKIP_BINARY_DOWNLOAD=1`：测试不需要 Electron 运行时。

### `.github/workflows/release.yml`（推送 `v*` 标签或手动）

- `rust-runtime`：6 个目标（`x86_64/aarch64` × `linux-gnu / windows-msvc / apple-darwin`）的 release 二进制，打包为
  `.tar.gz` / `.zip` 并附 sha256。Linux arm64 用 `ubuntu-24.04-arm`；macOS x64 在 arm64 runner 上交叉编译。
- `desktop`：`pnpm bundle:desktop -- --os <os> --arch <arch>`，矩阵 win-x64、mac-arm64、mac-x64（arm64 runner 交叉打包）、linux-x64；
  `ZCODE_BUNDLE_RUST_AGENT=1` 把 Rust runtime 放进 `resources/glm`（仍需 `ZCODE_AGENT_SERVER_RUNTIME=zcode-cli-rust` 才会选用，默认 Node）；
  `ZCODE_SKIP_REMOTE_ASSETS=1` 不打远程部署资产；Linux 额外安装 `rpm`、`libarchive-tools`（rpm/pacman 目标）。
- `publish`：仅标签触发，汇总全部产物创建**草稿** GitHub Release（人工确认后发布）。
- Electron 与 electron-builder 二进制镜像在 CI 中改为官方 GitHub 地址（仓库默认镜像面向国内网络）。

## 签名

- 当前全部未签名：macOS 未签名未公证（用户确认），Windows 未签名。
- `prepare-rust-agent.mjs` 的 macOS 规则改为跟随应用的签名开关：`ZCODE_ENABLE_MAC_SIGN=1` 时用同一身份
  （`ZCODE_RUST_CODESIGN_IDENTITY` / `APPLE_SIGNING_IDENTITY` / `CSC_NAME`，缺失即失败）以 hardened runtime 签名；
  未开启时整个应用都不签名，Rust 二进制保持未签名（此前的「macOS 必须提供身份」会让未签名 CI 构建直接失败）。
- 以后开启签名：在仓库 secrets 配置证书与 Apple 凭据，工作流设置 `ZCODE_ENABLE_MAC_SIGN=1` 等变量即可，无需改脚本。

## 本机准备工作

- 测试文件此前从未在 CI 中做类型检查，`tsconfig.zcode-cli-rust.json` 下有 28 处类型错误（均为测试代码的类型标注问题，不影响行为），
  已修复，否则 CI 在 `test:zcode-cli-rust` 的 tsc 步骤就会失败。
- 本机已验证：YAML 语法、全仓 `architecture:check`、`lint`、`typecheck`、测试 tsconfig 类型检查、受影响用例。

## 未验证

工作流本身尚未在 GitHub 上运行过（本机无法执行 Actions）。首次运行可能暴露：Linux/macOS 上此前从未跑过的 App 集成用例的平台差异、
MSVC 目标的编译差异、桌面打包在各 runner 上的环境依赖。首次推送后应以实际运行结果为准逐项修正。
