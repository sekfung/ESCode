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

### `.github/workflows/release.yml`（推送 `v*` 标签、手动，或 `feat/**` 上改动发布链路时）

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

## GitHub 实测结果（2026-09-25）

- 首次运行（36096673890）：init-prompt 语料在 Linux/macOS 上路径分隔符漂移、Windows 上 rpc 源码类型错误（干净检出缺少 `dist/*.d.ts`）。
  修复：语料统一 `/`；测试前 `tsc -b` 构建依赖包。
- 第二次（36097526844）：三平台均 210/243，27 个失败原因相同，无平台特有失败——干净检出没有
  `apps/zcode-cli/packages/cli/dist/zcode.cjs`（差分用例的 Node 一侧），以及工具定义对照表缺 plan 工具。
  修复：缺失时用 `scripts/build-desktop-agent-cli.mjs` 构建；对照表补 EnterPlanMode/ExitPlanMode。
- 第三次（36098587168）：checks 与 Windows（MSVC）/ macOS（arm64）/ Linux 测试**全部通过**。
- 2026-09-26（36209338763，提交 3b9b23b）：checks 与三平台测试全部通过。
  前一次运行在 Windows/macOS 上，因 PDF 路径未做 realpath 失败，已由 2b74cfe 修复。

## 发布链路实测（2026-09-25，run 36099990554，分支触发，未发布）

- 6 个 Rust runtime 目标全部构建成功（每个压缩包约 7–8 MB，附 sha256）。
- 4 个桌面安装包全部成功：win-x64（148 MB）、mac-arm64（362 MB）、mac-x64（373 MB）、linux-x64（deb/rpm/AppImage/pacman，共 572 MB）。
- 抽查：linux pacman 包 `resources/glm/` 同时含 `zcode-cli-rust` 与 Node `zcode.cjs`；
  `x86_64-pc-windows-msvc` 压缩包 sha256 校验通过，在本机 Windows 11 运行 `--help` 与 `app-server --stdio` 启动握手正常。
- `publish` 按设计跳过（非标签）。

## 未验证

- 标签触发的草稿 Release 未实际创建（打版本标签由维护者决定）。
- 安装包未在 macOS/Linux 真机上安装运行；macOS 未签名包需用户手动放行。
