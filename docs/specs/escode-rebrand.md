# 产品更名为 ESCode（品牌、包名、路径与环境变量）

2026-10-10，用户要求：把产品从 ZCode 更名为 ESCode；代码和图标里不再出现 ZCode；但不得修改源代码的归属权，并且必须继续符合 Apache-2.0 授权。

本 spec 定义改名范围、映射规则、必须保留的归属内容、图标方案和验证方式，作为本次机械改名的依据。

## 一、范围

仓库共 7975 个受版本控制的文件，其中 3606 个文件包含 `zcode`（大小写不敏感），约 3.1 万处；`ZCode` 12117 处、`zcode` 15143 处、`ZCODE` 3667 处。改名覆盖三层：

| 层面   | 内容                                                                                      | 示例                                                                                                 |
| ------ | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| 品牌   | 产品名、界面文案、文档、README、安装包身份                                                | `ZCode` → `ESCode`；`productName`、`appId`、Linux 可执行名                                           |
| 结构   | 包名、目录、文件名、Rust crate、工作区                                                    | `@zcode/*` → `@escode/*`；`apps/zcode-cli` → `apps/escode-cli`                                       |
| 运行时 | 环境变量、配置路径、存储目录、协议与 IPC 名称、二进制名、类型与函数名、CSS 属性、诊断标记 | `ZCODE_ENV` → `ESCODE_ENV`；`~/.zcode` → `~/.escode`；`zcodeTaskService.ts` → `escodeTaskService.ts` |

小写、首字母大写、全大写三种拼写按同一规则映射：`ZCode`→`ESCode`、`zcode`→`escode`、`ZCODE`→`ESCODE`。含 `zcode` 的路径逐段改名（307 个文件仅文件名含 `zcode`，其余由目录改名覆盖）。

## 二、必须保留（不属于品牌改名）

按 Apache-2.0 第 4 条，再分发时必须保留版权、专利、商标与归属声明，并保留对修改的声明。以下内容保持原样，不参与 `zcode` 替换：

1. `LICENSE` 全文，包括附录中的 `Copyright 2026 Z.AI Co., Ltd`。
2. `third-party/**`（`upstream/**`、`inventory.json`、`native-search/licenses/**`、`runtime/*.txt`）等按字节保护的上游许可原文。
3. 上游派生文件中的修改声明行：`Modified by ZCode: local integration, formatting and adaptations.`（`.agents/skills/**`、`packages/ui/src/components/ai-elements/**` 等）。这些声明记录的是上游项目对该文件的修改事实，改成 ESCode 属于篡改归属。
4. 第三方补丁的出处标记 `zcode patch:` 与 `- <pkg>: modified by ZCode; the changes are recorded in patches/...`（`THIRD-PARTY-NOTICES.md` 由 `scripts/generate-third-party-notices.mjs` 生成）。生成器改为保留一个显式常量（上游修改方名称），不随品牌改名。
5. Z.AI 运营的服务端点：`zcode.z.ai`、`cdn-zcode.z.ai`（含正则转义与大小写变体）。这是外部服务的地址，不是本产品品牌；改名会同时破坏登录、模型网关与分享链路，且我们并不拥有对应域名。代码中按“上游服务地址”保留，并在本 spec 与本轮变更说明中登记。
6. 第三方依赖名、上游仓库 URL、`Z.AI`/`z.ai` 等第三方标识。

## 三、影响与已知后果

- 安装包身份、`appId`、Linux 包名与可执行名改为 ESCode 命名空间；Windows 开发态 AUMID 从 `cn.aminer.zcode` 改为 `dev.escode.app.dev`，与正式 `appId` 同源。
- 环境变量整体改名（379 个 `ZCODE_*`），配置目录 `~/.zcode` → `~/.escode`，插件目录 `.zcode-plugin` → `.escode-plugin`，CLI 数据库 `~/.zcode/cli/db/db.sqlite` → `~/.escode/cli/db/db.sqlite`。
- **不做数据迁移**：Electron 的 `userData` 目录随产品名变化，旧安装的设置与会话不会自动迁移。按「不增加兜底分支」的既有约定，迁移作为独立议题，不在本次改名内实现。
- `pnpm-lock.yaml`、`apps/zcode-cli/pnpm-lock.yaml`、`apps/zcode-cli-rust/Cargo.lock` 随包名、crate 名与补丁内容变化重新生成；`patches/*.patch` 内容变化会改变其哈希，需要重算。
- `third-party/inventory.json` 与 `THIRD-PARTY-NOTICES.md` 记录被复制组件的路径与输入哈希，改名后必须用现有生成器重建。

## 四、图标

- 品牌资产：`public/logo/icons/**`、`packages/desktop/build/icon*.{ico,icns,png}`、`packages/desktop/build/icons/*.png`、`packages/ui/src/assets/Z.svg`（暗色空状态字标）。
- 方案：以新的 ESCode 标记（几何 `E` 字标，沿用现有深色渐变风格）为新源；用 electron-builder 自带的 `app-builder` 从 1024px PNG 生成 `ico`/`icns` 与尺寸集，替换上述资产；`packages/ui/src/assets/Z.svg` 改为 ESCode 标记并改名。
- 界面中的“产品 logo”引用（`packages/ui/src/App.tsx`、`WindowsTopLeftLogo.tsx`、`WorkspaceSidebar/WorkspaceSidebarCollapsedRail.tsx` 使用 `provider-icons/logo-zai.svg`）改为指向自有标识；`logo-zai.svg` 作为 Z.AI 模型提供商图标保留。
- `ZCodeAboutLogo.tsx`/`ZCodeWordmarkLogo` 随内容改名，作为第三方法务声明中记录的被复制文件，改名后重建 inventory。

## 五、合规

- 保留 `LICENSE` 与全部上游版权声明（第 4(c) 条）。
- 保留 NOTICE 类文件与第三方许可清单，随产物分发的行为不变（第 4(d) 条）。
- 在 `NOTICE.md` 增加 ESCode 衍生作品声明：说明本仓库是 ZCode 的衍生作品、本次改名与代码修改范围、上游版权仍归 Z.AI Co., Ltd，并保留原有全部声明（第 4(b)、§4(d) 增补条款）。
- 不主张 ZCode 相关商标；仅按第 6 条在描述来源的范围内保留 Z.AI 服务地址与上游项目名。

## 六、验证

1. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`。
2. `pnpm check:zcode-cli-rust`（边界、fmt、clippy）与 `pnpm test:zcode-cli-rust`（生成资产 `--check`、cargo test）。
3. `node scripts/generate-third-party-notices.mjs`（重建 inventory 与声明）后确认声明校验通过。
4. 残留审计：列出改名后仍含 `zcode`（大小写不敏感）的文件与行，逐条归入第二节的保护类别或修复；报告落到 `docs/reports/escode-rebrand-audit-2026-10-10.md`。
5. 图标产物可被 electron-builder 读取（`pnpm --filter @zcode/desktop run bundle` 的图标校验路径）。

## 七、提交切分

1. spec（本文件）。
2. 结构与内容改名（包名、路径、环境变量、文案、lockfile）。
3. 图标与 logo 引用。
4. NOTICE 增补 + 重建第三方声明/清单 + 残留审计报告。
