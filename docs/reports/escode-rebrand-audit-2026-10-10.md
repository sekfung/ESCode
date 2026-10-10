# ESCode 更名执行与残留审计（2026-10-10）

对应 spec：[docs/specs/escode-rebrand.md](../specs/escode-rebrand.md)。本文记录实际执行结果、残留 `zcode` 的分类与验证证据。

## 零、目标要求对照

| 目标要求               | 状态                                                                                                              | 证据                                                                                                                                                |
| ---------------------- | ----------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| 产品更名为 ESCode      | 完成                                                                                                              | 3476 个文件、14 个目录、566 个文件名改名；包名/环境变量/安装包身份/文案全部切换；[第一节](#一执行结果)                                              |
| 代码不出现 ZCode       | 品牌标识范围内完成；剩余仅上游归属声明、Z.AI 服务端点、`bizCode` 误报与说明文档（[第二节](#二保留-zcode-的类别)） | 文本分类审计 280 行全部归类、无未分类项；二进制字节扫描 0 处；类型检查/Lint/架构/Rust 全套通过                                                      |
| 图标不出现 ZCode       | 完成                                                                                                              | 以改名前基线逐 blob 清点 2376 个图片资产，品牌资产全部替换；打包产物图标与生成物 sha256 一致；DMG 背景与 Dock 图标已补修                            |
| 不得修改源代码的归属权 | 完成                                                                                                              | `LICENSE` 未改动（`Copyright 2026 Z.AI Co., Ltd`）；`Modified by ZCode`/`zcode patch:`/`modified by ZCode` 原样保留；产物元数据双署名               |
| 必须遵循 Apache-2.0    | 完成                                                                                                              | 安装包随附 `LICENSE`、`NOTICE.md`、`THIRD-PARTY-NOTICES.md`（第 4(a)(d) 条）；NOTICE 第五节登记本次修改（第 4(b) 条）；第三方声明与清单由生成器重建 |

两条要求在「上游修改声明」与「第三方服务地址」上存在冲突：前者受归属权约束必须保留，后者属外部服务标识、改名会破坏登录/网关/插件市场/更新。口径与回退路径见 [spec 第二节的裁定记录](../specs/escode-rebrand.md#裁定记录2026-10-10)；该裁定已两次请求用户确认但未获答复，按推荐口径执行并登记为可回退假设。

## 一、执行结果

| 项目         | 数量                                                                                                                                                                                                                               |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 内容改名文件 | 3476（`ZCode`→`ESCode`、`zcode`→`escode`、`ZCODE`→`ESCODE`、`Zcode`→`Escode`）                                                                                                                                                     |
| 目录改名     | 14（`apps/escode-cli`、`apps/escode-cli-rust`、`packages/escode-cua`、`packages/escode-server-cli`、`packages/shared/src/escode-protocol{,-v4}`、`packages/services/src/escode-agent`、`packages/services/src/escode-session` 等） |
| 文件改名     | 566（含 `scripts/generate-escode-cli-rust-*.mjs`、`packages/ui/src/components/ui/ESCodeAboutLogo.tsx`）                                                                                                                            |
| 包名         | `@zcode/*`→`@escode/*`、`zcode-cli`→`escode-cli`、Rust crate `zcode-cli-*`→`escode-cli-*`                                                                                                                                          |
| 环境变量     | 379 个 `ZCODE_*`→`ESCODE_*`；配置目录 `~/.zcode`→`~/.escode`，插件目录 `.zcode-plugin`→`.escode-plugin`                                                                                                                            |
| 安装包身份   | `appId dev.zcode.app*`→`dev.escode.app*`、`productName ZCode`→`ESCode`、Linux 包名/可执行名 `escode`；Windows 开发态 AUMID 改为 `dev.escode.app.dev`                                                                               |
| 图标         | 重新生成圆角方块 + 斜体 E 字标（`packages/desktop/build/icon*`、`build/icons/*`、`public/logo/icons/*`、`packages/web/public/favicon.ico`、`index.html` 内联 32px favicon 与 `packages/ui/src/assets/{E.svg,app-logo.svg}`）       |
| 界面字标     | `ESCodeAboutLogo` 标记与 `ESCodeWordmarkLogo`（ZCODE→ESCODE，新造 S 字形）；产品 logo 引用改指 `app-logo.svg`，Z.AI 提供商图标 `logo-zai.svg` 保留                                                                                 |
| lockfile     | 根 `pnpm-lock.yaml` 用 pnpm 重算；`apps/escode-cli/pnpm-lock.yaml` 同步改名；`apps/escode-cli-rust/Cargo.lock` 用 cargo 重算                                                                                                       |
| 第三方声明   | `third-party/copied-components.json` 路径更新 + 重新生成 `third-party/inventory.json` 与 `THIRD-PARTY-NOTICES.md`（1212 个 npm 版本、8 个复制组件、18 个原生归档）                                                                 |

## 二、保留 `zcode` 的类别

按 Apache-2.0 第 4 条与「不得修改源代码归属权」的要求，以下内容不参与品牌改名。计数口径：除本审计报告自身外共 **234 行、179 个文件**（本文档引用旧名会随编辑变动，故不计入）。

| 类别         | 数量 | 说明                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| ------------ | ---- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 上游修改声明 | 158  | `Modified by ZCode: local integration, formatting and adaptations.`（`.agents/skills/**`、`packages/ui/src/components/ai-elements/**` 等 113 处）、`modified by ZCode`（`THIRD-PARTY-NOTICES.md` 3 处，由 `scripts/generate-third-party-notices.mjs` 的 `UPSTREAM_PATCH_AUTHOR` 常量生成）、`zcode patch:`（`patches/*.patch` 4 处）、`scripts/generate-third-party-notices.mjs` 常量注释                                                                                                                                         |
| 上游服务地址 | 41   | `zcode.z.ai`、`cdn-zcode.z.ai`（含正则转义与大小写变体）：`.env.example`、`config/provider/escode-builtin.json`、Rust `mcp_official_auth.rs`/`client_headers.rs`/`official_plugins.json`、官方插件定义与 marketplace、`productDocs`、安装包元数据等 21 个文件。这些是 Z.AI 运营的外部服务端点，不是本产品品牌；其中 5 个代码默认值已可用环境变量覆盖，2 个（Z.AI / BigModel provider 的 `tokenUrl`）属第三方 provider 自身端点、任何口径下都不应改；逐条迁移清单见 [spec 裁定记录](../specs/escode-rebrand.md#裁定记录2026-10-10) |
| 文档说明     | 51   | `NOTICE.md` 第五节的衍生作品与修改声明、本 spec 的裁定记录、本审计报告对旧名的引用                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 非品牌误报   | 6    | `bizCode`（业务错误码字段名），不含品牌含义，未改名                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |

除上述类别外，全仓没有其它 `zcode` 残留。改名后 `THIRD-PARTY-NOTICES.md`、`third-party/inventory.json` 中第三方许可与版权文本零丢失（对比仅新增 serial 相关依赖条目与三处复制组件路径）。

补充：Rust 的官方 MCP 鉴权差分语料原本用生产域名当匹配参数，已改为合成域名 `official.example.test`（`crates/domain/tests/fixtures/mcp_official_auth_corpus.json` 与生成器同步），只保留 1 行对编译期内置默认值的断言；域内 Rust 用例 `origin_trust_matches_ts`、`escode_origin_matches_ts` 等 6 项仍全部通过。

口径说明：上表第一、二类是「代码不出现 ZCode」与「不得修改源代码归属权」的直接冲突点，本次按豁免口径执行（保留上游修改声明与 Z.AI 服务端点），理由与零残留时的迁移清单见 [spec 第二节的裁定记录](../specs/escode-rebrand.md#裁定记录2026-10-10)。该口径在用户另行裁定前有效；端点本身已可通过 `ESCODE_BASE_URL`、`ESCODE_CDN_BASE_URL` 等环境变量覆盖为自建服务，无需改代码。

## 三、验证证据

| 检查                                                | 结果                                                                                                                                                                                                                                                                               |
| --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm typecheck`                                    | 通过（修复 1 处被二进制检测跳过的导入：`packages/ui/src/hooks/useWorkflowRunNodeResult.ts` 含 NUL 分隔符，改名脚本按二进制跳过）                                                                                                                                                   |
| `pnpm lint`                                         | 通过（90 warnings / 0 errors，与改名前的既有基线一致）                                                                                                                                                                                                                             |
| `pnpm architecture:check --changed`                 | 通过：violations 0、baseline 0、new 0                                                                                                                                                                                                                                              |
| `cargo fmt --check`（escode-cli-rust）              | 通过（改名改变 `use escode_cli_*` 的字典序，35 处 import 顺序差异已用 `cargo fmt` 应用）                                                                                                                                                                                           |
| `pnpm test:escode-cli-rust`（GNU 目标）             | cargo 单元测试全部通过；App 集成套件 400 项：390 通过、9 跳过、1 失败（既有失败，见下）                                                                                                                                                                                            |
| `cargo clippy --all-targets -- -D warnings`         | **既有失败**：2 处 lint 与改名无关——`collapsible_match`（`crates/core/src/app/auxiliary.rs:207`，`match` 分支内的 `if`）与 `nonminimal_bool`（`crates/tools/src/plugin_validate_mcp.rs:276`，`!text(field).is_some_and(\|v\| !v.is_empty())`）；两段代码在改名前（HEAD~2）逐字存在 |
| 生成资产重建                                        | `scripts/generate-escode-cli-rust-*.mjs` 全部 28 个生成器按 TS 源重跑；`plugin_defaults.json`、`official_plugins.json` 等因 ID 改名导致排序变化，已由生成器重写；`--check` 一致                                                                                                    |
| `node scripts/check-escode-cli-rust-boundaries.mjs` | **既有失败**：`tools/src/mcp_hub.rs`（410 行）与 `tools/src/tools.rs`（403 行）超过 400 行上限；两者在 HEAD 同样是 410/403 行，与本次改名无关                                                                                                                                      |

App 集成套件中唯一失败的 `packages/services/tests/escode-cli-rust-options.test.ts` 也是既有问题：它 `import` 未在工作区中声明的 `@escode/model-option-map`，`packages/services/tsconfig.json` 没有该 paths 映射（改名前同样没有，`@zcode/model-option-map` 的同一断言在改名前代码上同样报 `ERR_MODULE_NOT_FOUND`），运行期解析一直依赖不存在的 node_modules 链接。改名未引入该失败，也未扩大范围。

### Rust 验证环境

- 本机未安装 Windows SDK（`C:\Program Files (x86)\Windows Kits\10\Lib` 不存在），MSVC 目标缺少 `kernel32.lib` 等系统库无法链接；Git Bash 的 `/usr/bin/link` 还会与 MSVC `link.exe` 抢名。
- 仓库文档（[rust-ci.md](../specs/rust-ci.md)）已记录本机此前的 Rust 结论取自 GNU 目标；本次沿用 `x86_64-pc-windows-gnu`，命令：
  `PATH=/c/msys64/mingw64/bin:$PATH RUSTUP_TOOLCHAIN=1.95.0-x86_64-pc-windows-gnu pnpm test:escode-cli-rust`
  （`if let` 守卫需要 1.95 工具链，本机 stable 为 1.93，该语法在 HEAD 已存在。）
- App 集成套件按 `target/debug/` 取产物，而带 `--target` 的构建落在 `target/x86_64-pc-windows-gnu/debug/`；为不改动测试，验证时把新产物同步到 `target/debug/`（`target/` 不入库）。首次运行曾因残留的 9 月 30 日旧产物（内含 `zcode-credential-fallback`）导致 2 项凭据差分测试误报，同步后通过。
- 环境限制记录：本机 `pnpm -r ls --depth Infinity` 会因句柄耗尽报 `EMFILE`（与改名无关），生成 `THIRD-PARTY-NOTICES.md` 时改用逐项目采集的依赖图 JSON 临时注入，生成器与仓库代码未保留该改动。

### 桌面安装包冒烟（Windows x64）

`ESCODE_BUNDLE_RUST_AGENT=0 ESCODE_SKIP_REMOTE_ASSETS=1 pnpm bundle:desktop -- --os win --arch x64`，退出码 0，产物 `packages/desktop/dist/ESCode Preview-3.14.0-win-x64_TEST.exe`（142.8 MiB，体积审计上限 500 MiB 通过）。核对结果：

| 检查项            | 结果                                                                                                                                            |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| 产物身份          | `win-unpacked/ESCode Preview.exe`，`ProductName=ESCode Preview`、`CompanyName=ESCode`                                                           |
| 更新元数据        | `latest.yml` 指向 `ESCode Preview-3.14.0-win-x64_TEST.exe`，无旧名                                                                              |
| 图标              | `resources/icon.png`、`icon_windows.png` 与 `packages/desktop/build/` 生成物 sha256 一致                                                        |
| 内置 Agent bundle | `resources/glm/escode.cjs`                                                                                                                      |
| ASCII/合规文件    | `resources/LICENSE`（仍是 `Copyright 2026 Z.AI Co., Ltd`）、`resources/NOTICE.md`（含 ESCode 衍生作品声明）、`resources/THIRD-PARTY-NOTICES.md` |

### 运行时冒烟（server 与 web）

此前只对服务端与 Web 做过类型检查，未真正启动过运行时；本轮补上：

- **后端**：`pnpm --filter "@escode/server..." build`（产出 `dist/remote/escode-server.cjs`）后以 `PORT=3458 ESCODE_DATA_BASE_DIR=<临时目录> node packages/server/dist/entry-http.js` 启动，`GET /api/server-info` 返回 200：
  `{"serverId":"DESKTOP-U6EQEHV","version":"3.14.0","protocolVersion":1,"authRequired":false,"workspaces":[{"path":"…\\ESCode","label":"ESCode"}],…}`。
  启动日志的 scope 为 `escode-server:http`，Provider Registry 的 configRevision 前缀为 `escode-builtin:30:…`。首次请求返回 503 是本机 HTTP 代理拦截所致（响应头带 `Proxy-Connection`），`--noproxy` 绕过代理后为 200。
- **Web**：`pnpm --filter @escode/web dev` 后取 `http://localhost:5173/`（本机解析为 IPv6 `::1`）返回 200，页面含 `<title>ESCode</title>`、`escode-theme` 存储键、内联 favicon、`--escode-bootstrap-bg` 与 `data-escode-*` 属性，响应 HTML 中 `zcode` 出现 **0 次**。
- 两个冒烟进程已终止，临时数据目录已清理。

冒烟暴露并修复了两处合规缺口（提交 `254b2b1d`）：版权字段原由 `author` 推导为 `Copyright © 2026 ESCode`，把上游权利人一并替换，现改为显式双署名；`resources` 原先只带第三方声明，现随附项目 `LICENSE` 与 `NOTICE.md`（Apache-2.0 第 4(a)、4(d) 条）。

### CLI 与文档链接核验

- **CLI 产物**：`node apps/escode-cli/packages/cli/dist/escode.cjs --version` → `0.16.9`；`--help` 输出 `escode 0.16.9`、`Run the ESCode Protocol stdio app server` 等，全文 `zcode` 出现 **0 次**；`doctor` 自述 `process: escode-cli`、`version: 0.16.9`。
- **文档链接**：对全部受控 markdown 的相对链接做存在性核验并与改名前基线对比（基线 44 处、当前 45 处）。逐条核对后确认**没有因改名新断的链接**：两边的差异项是同一批既有问题的承载文件换了路径（`memory_section.md` 的 `file.md` 占位、`dependencies/README.md` 指向并不存在的 `third-party/README.md`、skill 内的 `file:///C:/Users/test/…` 示例、`docs/rust-migration.md` 以仓库根相对路径指向目录），其中多项为检查器对目录、示例 URL 与仓库根相对路径的误判。

### 图标与二进制资产核验

- 以改名前基线 `origin/feat/rust-runtime` 逐 blob 比对全部 2376 个图片资产：28 个产品品牌资产被替换；其余 2347 个未变动项里，5 个非第三方项经目视核对是 macOS Finder/Terminal、飞书图标与通用箭头。
- 清点发现并修复了两处漏网（提交 `285836ba`）：`packages/desktop/build/dmg_background(.@2x).png` 中央印着 ZCODE 大字（macOS 安装窗口背景，已按同版式重绘为 ESCode 字标与箭头）；`public/icon_512@2x.png` 在改名前与应用图标同内容（更新弹窗里的 macOS Dock 图标，已换成新的 ESCode 应用图标）。
- 全部受版本控制的二进制文件按字节扫描 `zcode`（大小写不敏感）：**0 处**。
- 渲染层重建（`pnpm --filter @escode/desktop build:no-runtime-assets`）退出码 0，产物 `out/renderer/assets/icon_512@2x-15FB8BAe.png` 与新 Dock 图标 sha256 一致。
- 未覆盖：macOS DMG 实际打包（本机为 Windows，DMG 背景只在 macOS 打包链路生效）。

未覆盖：`prepare:rust-agent`（需要 MSVC 目标，本机缺 Windows SDK）与 `prepare:remote-assets`（`ESCODE_SKIP_REMOTE_ASSETS=1`）被跳过，因此安装包内不含 Rust runtime 与远程部署资产；界面 E2E 仍未执行。

## 四、已知后果与后续事项

- **不做数据迁移**：`productName`、`~/.zcode`→`~/.escode`、`ZCODE_*`→`ESCODE_*` 均不兼容旧安装；旧用户的设置、会话与 CLI 数据库不会自动迁移，需要时另立议题。
- **上游服务端点保持不变**：`zcode.z.ai`、`cdn-zcode.z.ai` 仍指向 Z.AI 服务；在自建后端与 CDN 之前，登录、模型网关、插件市场与自动更新仍依赖上游服务。
- **图标为程序化重建**：应用图标沿用「深色圆角方块 + 斜体字标」构图，安装包图标沿用纸箱构图，均为按几何重新绘制，未复用上游设计源文件。
- **未执行**：界面 E2E（本机无对应套件与驾驶环境）；桌面打包冒烟已执行，但跳过 Rust runtime 与远程资产两步，原因见上节。
