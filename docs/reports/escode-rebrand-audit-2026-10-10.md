# ESCode 更名执行与残留审计（2026-10-10）

对应 spec：[docs/specs/escode-rebrand.md](../specs/escode-rebrand.md)。本文记录实际执行结果、残留 `zcode` 的分类与验证证据。

## 一、执行结果

| 项目         | 数量                                                                                                                                                                                                                         |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------- |
| 内容改名文件 | 3476（`ZCode`→`ESCode`、`zcode`→`escode`、`ZCODE`→`ESCODE`、`Zcode`→`Escode`）                                                                                                                                               |
| 目录改名     | 14（`apps/escode-cli`、`apps/escode-cli-rust`、`packages/escode-cua`、`packages/escode-server-cli`、`packages/shared/src/escode-protocol{,-v4}`、`packages/services/src/escode-agent                                         | escode-session` 等） |
| 文件改名     | 566（含 `scripts/generate-escode-cli-rust-*.mjs`、`packages/ui/src/components/ui/ESCodeAboutLogo.tsx`）                                                                                                                      |
| 包名         | `@zcode/*`→`@escode/*`、`zcode-cli`→`escode-cli`、Rust crate `zcode-cli-*`→`escode-cli-*`                                                                                                                                    |
| 环境变量     | 379 个 `ZCODE_*`→`ESCODE_*`；配置目录 `~/.zcode`→`~/.escode`，插件目录 `.zcode-plugin`→`.escode-plugin`                                                                                                                      |
| 安装包身份   | `appId dev.zcode.app*`→`dev.escode.app*`、`productName ZCode`→`ESCode`、Linux 包名/可执行名 `escode`；Windows 开发态 AUMID 改为 `dev.escode.app.dev`                                                                         |
| 图标         | 重新生成圆角方块 + 斜体 E 字标（`packages/desktop/build/icon*`、`build/icons/*`、`public/logo/icons/*`、`packages/web/public/favicon.ico`、`index.html` 内联 32px favicon 与 `packages/ui/src/assets/{E.svg,app-logo.svg}`） |
| 界面字标     | `ESCodeAboutLogo` 标记与 `ESCodeWordmarkLogo`（ZCODE→ESCODE，新造 S 字形）；产品 logo 引用改指 `app-logo.svg`，Z.AI 提供商图标 `logo-zai.svg` 保留                                                                           |
| lockfile     | 根 `pnpm-lock.yaml` 用 pnpm 重算；`apps/escode-cli/pnpm-lock.yaml` 同步改名；`apps/escode-cli-rust/Cargo.lock` 用 cargo 重算                                                                                                 |
| 第三方声明   | `third-party/copied-components.json` 路径更新 + 重新生成 `third-party/inventory.json` 与 `THIRD-PARTY-NOTICES.md`（1212 个 npm 版本、8 个复制组件、18 个原生归档）                                                           |

## 二、保留 `zcode` 的类别（共 243 行，177 个文件）

按 Apache-2.0 第 4 条与「不得修改源代码归属权」的要求，以下内容不参与品牌改名：

| 类别         | 数量 | 说明                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------ | ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 上游修改声明 | 160  | `Modified by ZCode: local integration, formatting and adaptations.`（`.agents/skills/**`、`packages/ui/src/components/ai-elements/**` 等 113 处）、`modified by ZCode`（`THIRD-PARTY-NOTICES.md` 3 处，由 `scripts/generate-third-party-notices.mjs` 的 `UPSTREAM_PATCH_AUTHOR` 常量生成）、`zcode patch:`（`patches/*.patch` 4 处）、`scripts/generate-third-party-notices.mjs` 常量注释 |
| 上游服务地址 | 77   | `zcode.z.ai`、`cdn-zcode.z.ai`（含正则转义与大小写变体）：`.env.example`、`config/provider/escode-builtin.json`、Rust `mcp_official_auth.rs`/`client_headers.rs`/`official_plugins.json`、官方插件定义与 marketplace、`offPeakServerClient` 等。这些是 Z.AI 运营的外部服务端点，不是本产品品牌                                                                                            |
| 文档说明     | 3    | `NOTICE.md` 第五节对本仓库与上游关系、服务地址归属及本次修改的声明                                                                                                                                                                                                                                                                                                                        |
| 非品牌误报   | 6    | `bizCode`（业务错误码字段名），不含品牌含义，未改名                                                                                                                                                                                                                                                                                                                                       |

除上述类别外，全仓没有其它 `zcode` 残留。改名后 `THIRD-PARTY-NOTICES.md`、`third-party/inventory.json` 中第三方许可与版权文本零丢失（对比仅新增 serial 相关依赖条目与三处复制组件路径）。

## 三、验证证据

| 检查                                                | 结果                                                                                                                                                                                                                    |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --- | ----------------------------------------------------- |
| `pnpm typecheck`                                    | 通过（修复 1 处被二进制检测跳过的导入：`packages/ui/src/hooks/useWorkflowRunNodeResult.ts` 含 NUL 分隔符，改名脚本按二进制跳过）                                                                                        |
| `pnpm lint`                                         | 通过（90 warnings / 0 errors，与改名前的既有基线一致）                                                                                                                                                                  |
| `pnpm architecture:check --changed`                 | 通过：violations 0、baseline 0、new 0                                                                                                                                                                                   |
| `cargo fmt --check`（escode-cli-rust）              | 通过（改名改变 `use escode_cli_*` 的字典序，35 处 import 顺序差异已用 `cargo fmt` 应用）                                                                                                                                |
| `pnpm test:escode-cli-rust`（GNU 目标）             | cargo 单元测试全部通过；App 集成套件 400 项：390 通过、9 跳过、1 失败（既有失败，见下）                                                                                                                                 |
| `cargo clippy --all-targets -- -D warnings`         | **既有失败**：2 处 lint 与改名无关——`collapsible_match`（`crates/core/src/app/auxiliary.rs:207`，`match` 分支内的 `if`）与 `nonminimal_bool`（`crates/tools/src/plugin_validate_mcp.rs:276`，`!text(field).is_some_and( | v   | !v.is_empty())`）；两段代码在改名前（HEAD~2）逐字存在 |
| 生成资产重建                                        | `scripts/generate-escode-cli-rust-*.mjs` 全部 28 个生成器按 TS 源重跑；`plugin_defaults.json`、`official_plugins.json` 等因 ID 改名导致排序变化，已由生成器重写；`--check` 一致                                         |
| `node scripts/check-escode-cli-rust-boundaries.mjs` | **既有失败**：`tools/src/mcp_hub.rs`（410 行）与 `tools/src/tools.rs`（403 行）超过 400 行上限；两者在 HEAD 同样是 410/403 行，与本次改名无关                                                                           |

App 集成套件中唯一失败的 `packages/services/tests/escode-cli-rust-options.test.ts` 也是既有问题：它 `import` 未在工作区中声明的 `@escode/model-option-map`，`packages/services/tsconfig.json` 没有该 paths 映射（改名前同样没有，`@zcode/model-option-map` 的同一断言在改名前代码上同样报 `ERR_MODULE_NOT_FOUND`），运行期解析一直依赖不存在的 node_modules 链接。改名未引入该失败，也未扩大范围。

### Rust 验证环境

- 本机未安装 Windows SDK（`C:\Program Files (x86)\Windows Kits\10\Lib` 不存在），MSVC 目标缺少 `kernel32.lib` 等系统库无法链接；Git Bash 的 `/usr/bin/link` 还会与 MSVC `link.exe` 抢名。
- 仓库文档（[rust-ci.md](../specs/rust-ci.md)）已记录本机此前的 Rust 结论取自 GNU 目标；本次沿用 `x86_64-pc-windows-gnu`，命令：
  `PATH=/c/msys64/mingw64/bin:$PATH RUSTUP_TOOLCHAIN=1.95.0-x86_64-pc-windows-gnu pnpm test:escode-cli-rust`
  （`if let` 守卫需要 1.95 工具链，本机 stable 为 1.93，该语法在 HEAD 已存在。）
- App 集成套件按 `target/debug/` 取产物，而带 `--target` 的构建落在 `target/x86_64-pc-windows-gnu/debug/`；为不改动测试，验证时把新产物同步到 `target/debug/`（`target/` 不入库）。首次运行曾因残留的 9 月 30 日旧产物（内含 `zcode-credential-fallback`）导致 2 项凭据差分测试误报，同步后通过。
- 环境限制记录：本机 `pnpm -r ls --depth Infinity` 会因句柄耗尽报 `EMFILE`（与改名无关），生成 `THIRD-PARTY-NOTICES.md` 时改用逐项目采集的依赖图 JSON 临时注入，生成器与仓库代码未保留该改动。

## 四、已知后果与后续事项

- **不做数据迁移**：`productName`、`~/.zcode`→`~/.escode`、`ZCODE_*`→`ESCODE_*` 均不兼容旧安装；旧用户的设置、会话与 CLI 数据库不会自动迁移，需要时另立议题。
- **上游服务端点保持不变**：`zcode.z.ai`、`cdn-zcode.z.ai` 仍指向 Z.AI 服务；在自建后端与 CDN 之前，登录、模型网关、插件市场与自动更新仍依赖上游服务。
- **图标为程序化重建**：应用图标沿用「深色圆角方块 + 斜体字标」构图，安装包图标沿用纸箱构图，均为按几何重新绘制，未复用上游设计源文件。
- **未执行**：桌面安装包实际打包冒烟（`pnpm bundle:desktop`）与界面 E2E；本机仅验证到类型、Lint、架构、Rust 边界与生成资产一致性。
