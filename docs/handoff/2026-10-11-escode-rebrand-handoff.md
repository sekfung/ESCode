# 交接手册：ZCode → ESCode 更名收尾（2026-10-11）

写给接手的 AI。读完本文即可继续，不需要翻历史对话。先读仓库根目录 `AGENTS.md`（必须遵守），再读本文。

## 0. 总目标与用户约束

用户目标（原话）：

1. 分析 main 改动，合并 main 分支，并重新对齐 node-runtime（Rust runtime 与 Node runtime 行为一致）。
2. 在不违反 Apache-2.0 协议的前提下，尽可能去除 ZCode 内容，并将产品名更名为 ESCode，包含图标。

用户的明确指示：

- 只在当前分支 `refactor/rebrand` 上工作。不要管其他分支（`refactor/escode-rebrand`、`wip/...`、`refacttor/rebrain`、旧 stash），那些是错的。
- **不需要 Z.ai（ZCode）的账号登录与 Coding Plan 套餐**。因此登录 / 套餐相关的外部契约（`zcode://` 回调、BigModel `appId=zcode`、Z.ai 托管页面的桥接名等）不做兼容，一律按 ESCode 改名。详见 `docs/specs/escode-rebrand.md`「不保留的 Z.ai 专属契约」。
- 提交信息结尾加：`Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`。只在本地提交，不要 push。

## 1. 当前进度

| 阶段 | 状态 |
| --- | --- |
| 合并 main（v3.15.1） | 已完成，提交 `0dd53574` |
| Rust runtime 对齐 v3.15.1 | 已完成，提交 `aff7afe2`；说明见 `docs/specs/rust-v3.15.1-drift.md` |
| ESCode 更名 | **已在工作区完成，未提交**（约 11,481 个变更：3,429 个纯重命名、2,814 个重命名加修改、约 5,232 个修改） |
| 更名后的测试修复 | **进行中**，剩余问题见第 4 节 |

`aff7afe2` 提交前的验证结果：typecheck、lint（0 错误）、CLI 31 包 typecheck、30 个漂移生成器检查都通过；Rust 测试 319 个通过；App 差分套件 400 个测试中 389 通过、9 跳过，另 2 个单独重跑通过（并发跑 cargo 时 CPU 争用导致超时）。

## 2. 更名做了什么（工作区，未提交）

- 批量替换（内容与路径）：`ZCODE→ESCODE`、`ZCode→ESCode`、`Zcode→Escode`、`zCode→esCode`、`zcode→escode`。目录如 `apps/zcode-cli` 已变为 `apps/escode-cli`，`apps/zcode-cli-rust` 已变为 `apps/escode-cli-rust`，`packages/zcode-server-cli` 已变为 `packages/escode-server-cli`。包作用域 `@escode/*`，crate `escode-cli-*`，环境变量 `ESCODE_*`。
- Apache-2.0：
  - `LICENSE` 未动。
  - 上游 `NOTICE.md` 原文移到 `third-party/upstream-zcode/NOTICE.md`。
  - 新 `NOTICE.md` 写明来源、修改声明与商标说明。
  - README 中英文的「项目声明」段已改写，并删除了 ZCode 社群（飞书 / Discord）链接。
- 图标：
  - 已替换：`packages/desktop/build/` 下全部 png / ico / icns、`icons/*.png`、`dmg_background*.png`，`public/logo/icons/*`、`public/icon_512@2x.png`、`packages/web/public/favicon.ico`。
  - 内联标识已改为 ESCode 斜体 E（中横强调色 `#22D3EE`）与 ESCODE 字标：
    - `ESCodeAboutLogo.tsx`、`RootStartupLoading.tsx`、`aboutWindow.ts`、`packages/desktop/src/renderer/index.html`、`packages/web/index.html`
    - 空状态：`packages/ui/src/assets/escode-outline.svg` 取代了 `Z.svg`
    - 应用小图标：`packages/ui/src/assets/escode-mark.svg`，App.tsx、WindowsTopLeftLogo、WorkspaceSidebarCollapsedRail 已改用它
- 外部契约的最终处理（以 `docs/specs/escode-rebrand.md` 为准）：
  - **保留 `zcode`**，原因是它们是真实服务地址或插件生态契约：
    - 主机名 `zcode.z.ai`、`cdn-zcode.z.ai`、`zcode.ai`
    - `zcode-plan` 路径
    - `x-zcode-*` 请求头
    - `zcode-websearch-client`
    - 官方插件市场 id `zcode-plugins-official`（需与插件 CDN 的 `marketplace.json` 中 `name` 一致）
    - 旧插件清单目录 `.zcode-plugin/`：作为 `.escode-plugin/` 之后的回退读取，TS 与 Rust 两侧都已实现，TS 加了测试
  - **不保留（已改名）**：
    - 深链只用 `escode://`；所有 `zcode://`（含 OAuth 回调）都已改为 `escode://`
    - BigModel `appId` 为 `escode`
    - Z.ai 托管页面的桥接、远控帧字段 `escode_type`、协议名 `ESCode Protocol`、MCP `_meta["escode/…"]`、遥测事件名
- 锁文件：`pnpm-lock.yaml` 已用 `pnpm install --no-frozen-lockfile` 更新；变化只来自补丁文件 hash 与包名。`Cargo.lock` 是文本替换，`cargo build --locked` 通过。
- 生成资产：全部用 `scripts/generate-escode-cli-rust-*.mjs` 重新生成，`--check` 全部通过。

辅助脚本在仓库外的 `C:\Users\sekfung\Projects\escode-rebrand-tools\`：

| 脚本 | 用途 | 状态 |
| --- | --- | --- |
| `rebrand.py` | 批量替换 | 已执行，**不要再跑** |
| `finish_paths.py` | 补齐嵌套路径改名 | 已执行，**不要再跑** |
| `logos.py` | 内联标识替换 | 已执行，**不要再跑** |
| `legacy_manifest.py` | `.zcode-plugin` 回退 | 已执行，**不要再跑** |
| `deeplink_revert.py` | 撤销 `zcode://` 深链别名 | 已执行，**不要再跑** |
| `icons/` | 图标生成器，`out/` 为产物 | 可在需要时重新生成图标 |

## 3. 已知坑（务必注意）

1. **`packages/ui/src/hooks/useWorkflowRunNodeResult.ts` 含字面 NUL 分隔符**，批量脚本把它当二进制跳过了，已手工改名。以后做批量文本替换时要单独检查这类文件。
2. **漂移生成器偶发 Windows 文件锁**（`UNKNOWN: open` / `Errno 22`）：重试即可。重试后必须再跑一次 `--check`。曾出现过生成文件残留旧内容、后来被覆盖的情况。
3. **测试日志带 ANSI 颜色码**：统计 `✔` / `✖` 之前先 `sed 's/\x1b\[[0-9;]*m//g'`。
4. Rust 在 Windows 上没有 MSVC，必须用 GNU 工具链：

   ```bash
   export PATH=/c/msys64/mingw64/bin:$PATH
   cargo +1.95.0-x86_64-pc-windows-gnu {build|test|check} --locked \
     --manifest-path apps/escode-cli-rust/Cargo.toml --target x86_64-pc-windows-gnu
   ```

   格式化只用 `cargo +1.95.0-x86_64-pc-windows-gnu fmt --manifest-path apps/escode-cli-rust/Cargo.toml`，不要在 crate 目录里跑裸 `cargo fmt`。
5. App 差分测试（`packages/services/tests/escode-cli-rust-*.test.ts`）的准备步骤：
   - 把 `apps/escode-cli-rust/target/x86_64-pc-windows-gnu/debug/escode-cli-rust.exe` 复制到 `apps/escode-cli-rust/target/debug/`。
   - examples 先 `cargo build --examples`，再把产出复制到 `target/debug/examples/`。
   - 用 `node scripts/build-desktop-agent-cli.mjs` 构建 Node CLI bundle，产物为 `apps/escode-cli/packages/cli/dist/escode.cjs`。
   - 运行：

     ```bash
     export TSX_TSCONFIG_PATH="$(pwd)/packages/services/tests/tsconfig.escode-cli-rust.json" ESCODE_TEST_REQUEST_MS=30000 ESCODE_TEST_WAIT_MS=30000
     node --import tsx --test --test-concurrency=1 --test-timeout=300000 --test-force-exit packages/services/tests/escode-cli-rust-*.test.ts
     ```

   - 全量约 2.5 小时。**不要与 cargo test 并行**，CPU 争用会造成假失败。
6. `pnpm fmt:check` 在全仓库有大量既有失败（`.agents/skills/*`、`docs/electron` 镜像等），与本任务无关。只对自己改过的文件跑 `npx oxfmt <files>`。
7. lint 当前为 0 错误、83 条警告（`aff7afe2` 时是 82 条，新增 1 条未定位，不阻塞）。
8. `pnpm install` 需要 `npm_config_manage_package_manager_versions=false`，否则会报 EPERM。CLI 子仓 typecheck 用：

   ```bash
   cd apps/escode-cli && ../../node_modules/.bin/turbo run typecheck
   ```

## 4. 剩余工作（按顺序）

### 4.1 分清哪些 vitest 失败是更名引入的

根目录 `npx vitest run` 当前有 38 个文件、117 个测试失败（日志：`C:\Users\sekfung\Projects\escode-rebrand-tools\vitest2.log`）。很多看起来是既有或环境问题，需要用基线判定：

- 基线 worktree 已建好并 `pnpm install` 完成：`C:\Users\sekfung\Projects\escode-baseline`（detached 在 `aff7afe2`，即更名前）。
- 在基线里跑同样的测试文件（路径换回 `zcode` 名）：

  ```bash
  cd /c/Users/sekfung/Projects/escode-baseline
  npx vitest run packages/ui/test/interfaceMode.test.ts packages/services/test/zcodeAgentProcessManager.test.ts ...
  ```

- 基线也失败的，是既有问题，不用修；只修更名后新出现的失败。
- 判定完成后执行 `git worktree remove --force /c/Users/sekfung/Projects/escode-baseline`。

当前失败文件，以及已判断出的方向：

**大概率是更名引入，需要修：**

- `packages/desktop/test/about.test.ts`：断言旧 `viewBox="0 0 256 218"`，改为新标识的 `0 0 228 196`，并核对断言里的其它路径片段。
- `packages/ui/test/pluginStoreListing.test.ts`：UI 由市场 id 推导展示名，得到 `Zcode Plugins Official`，测试期望 `ESCode Plugins Official`。市场 id 保留为 `zcode-plugins-official`，所以应在 UI 的展示名逻辑里给官方市场一个固定的 ESCode 展示名，而不是改测试。先查 `packages/ui/src` 中由 marketplace id 生成名字的地方，还要检查其他 UI 位置是否会露出 `Zcode`。
- `packages/escode-server-cli/test/packaging.test.ts`、`packages/services/test/skillsService.test.ts`、`packages/services/test/providerProvisioningAccountRefresh.test.ts`：多半是更名后排序变化（`escode-*` 排到前面）。确认后更新期望顺序。
- `packages/shared/test/escodeEndpoint.test.ts`：中继 `wss://zcode.z.ai/ws` 已恢复，核对测试期望。
- `packages/web/test/webRemoteControlBootstrapShell.test.ts`：内联 favicon 的 data URL 与新 `favicon.ico` 不一致，要么同步 `packages/web/index.html` 里的内联图标，要么按测试的比对方式更新。
- `packages/ui/test/providerLogoAssets.test.ts`：可能在比对 `public/logo/icons` 与打包产物的字节，或检查尺寸。新图标已复制到两处，查清楚比的是哪两个文件。
- `packages/desktop/test/desktopLinuxDeepLinkRegistration.test.ts`：已撤销双 scheme，重跑确认。

**大概率既有 / 环境问题，用基线确认：**

- `serial*.test.ts`：报 No test suite found。
- `interfaceMode`、`startPlan*`、`codingPlanEmbeddedWebview`：`localStorage` 为 undefined，与 Node 26 环境有关。
- `v4SessionPane*`
- `escodeAgentProcessManager`、`escodeAgentService`：进程树清理问题。
- `windowsInstaller*`：NSIS。
- `tsupConfig`、`tsupChunkIsolation`、`powershell-command`、`desktopMainIpcRemote*`、`subagentConfigProxy`、`codingPlanSubscriptionService`、`escodeAgentService.providerRegistry`

### 4.2 改完后全量验证（如实记录结果）

- `pnpm typecheck`、CLI turbo typecheck、`pnpm lint`
- `pnpm architecture:check --changed`
- 所有 `scripts/generate-escode-cli-rust-*.mjs --check`
- Rust `cargo test --workspace --no-fail-fast`（上次结果：319 个通过、0 失败）
- App 差分全量（最近一次：400 个测试中 364 通过、27 失败。之后修复了 plugin_defaults 陈旧生成物，16 个失败文件重跑 35/35 通过；但这次重跑早于「外部主机名恢复」与「撤销 zcode 深链」，需要再跑一次全量）
- 根 vitest，与基线对比

### 4.3 残留扫描

```bash
git grep -I -n -i zcode -- ':!third-party' ':!docs/electron' ':!docs/knip' ':!LICENSE' ':!NOTICE.md' ':!docs/specs/escode-rebrand.md' ':!docs/handoff'
```

（本手册自身大量提到 zcode，扫描时排除；收尾提交后可删除本手册，或保留作为记录。）

结果中只应出现 `docs/specs/escode-rebrand.md`「刻意保留」表里的项：主机名、`zcode-plan`、`x-zcode-*`、`zcode-websearch-client`、`zcode-plugins-official`、`.zcode-plugin`。另外检查 `git ls-files | grep -i zcode`，结果只应在 `third-party/`、`docs/electron`、`docs/knip` 下。

### 4.4 提交

- 只提交本任务的变更。
- 不要提交 `apps/escode-cli/packages/*/dist-types/` 的构建副产物，在 Windows 上构建后会有换行或 map 变化；提交前执行 `git checkout -- <路径>` 还原。
- 建议提交信息：

  ```text
  refactor: 产品更名为 ESCode

  - 包名、目录、crate、环境变量、协议名与深链 scheme 改为 ESCode
  - 图标、dmg 背景与应用内标识替换为 ESCode 标识
  - 上游 NOTICE 原文移至 third-party/upstream-zcode/，新 NOTICE 声明来源与修改
  - 保留外部服务主机名、插件市场 id 与旧插件清单目录回退（docs/specs/escode-rebrand.md）

  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```

- 提交后可删除仓库外的 `escode-rebrand-tools`（先确认图标生成器不再需要）和 `escode-baseline` worktree。

## 5. 关键文档

- `AGENTS.md`：仓库规则（spec 先行、真实报告验证结果、中文注释、日志规范等）。
- `docs/specs/escode-rebrand.md`：更名规范与外部契约清单（保留 / 不保留）。
- `docs/specs/rust-v3.15.1-drift.md`：Rust 对齐 v3.15.1 的逐项说明、已知缺口与夹具调整。
- `NOTICE.md`、`third-party/upstream-zcode/NOTICE.md`：许可署名。

## 6. 收尾完成记录（2026-10-11 晚，接手会话）

第 4 节全部完成。

**4.1 归属判定**：在基线 worktree（`aff7afe2`）复跑同一批 38 个失败文件，其中 10 个是更名引入（基线通过），其余 28 个基线上同样失败。10 个已全部修好：

- `desktop/about.test.ts`：About 内联标识 viewBox 改为新标识的 `0 0 228 196`。
- `ui/pluginStoreListing.test.ts`：官方市场展示名不再按 slug 推导；官方 id 的展示名固定为 `ESCode Plugins Official`（新增 `ESCODE_OFFICIAL_PLUGIN_MARKETPLACE_DISPLAY_NAME`），同时修掉 `resolveMarketplaceDisplayName` 会把 `zcode-plugins-official` 原始 id 当标题展示的真实泄漏。另修正 `escode-*` 排序位次变化后的期望顺序。
- `escode-server-cli/packaging.test.ts`、`services/skillsService.test.ts`、`services/providerProvisioningAccountRefresh.test.ts`：`escode-*` 排序位次变化后更新期望顺序。
- `shared/escodeEndpoint.test.ts`、`services/codingPlanSubscriptionService.test.ts`：测试里被误改的 `escode-plan` 路径还原为保留的 `zcode-plan`。
- `web/webRemoteControlBootstrapShell.test.ts`：`packages/web/index.html` 内联 favicon 与新 `public/logo/icons/32x32.png` 同步。
- `ui/providerLogoAssets.test.ts`：`model-provider-zai-app.png` 按清单里既有的「等同 Dock 应用图标」约定换成新 ESCode 图标。
- `desktop/tsupChunkIsolation.test.ts`：CDN 注入用例与纯模板函数输出对齐（样例域名统一为 `cdn.escode-ai.com`）。

**4.2 验证结果**：

| 检查 | 结果 |
| --- | --- |
| `pnpm typecheck` | 通过 |
| `pnpm lint` | 0 错误 / 82 警告（回到 `aff7afe2` 水平） |
| `pnpm architecture:check --changed` | OK，0 violations |
| 30 个 `scripts/generate-escode-cli-rust-*.mjs --check` | 全部通过（须 `node --import tsx <script> --check`） |
| Rust `cargo test --workspace --no-fail-fast` | 319 通过 / 0 失败 |
| CLI `turbo run typecheck` | 31/31（首次因 Windows 文件锁失败，重试通过） |
| 根 `npx vitest run` | 1940 文件 / 18131 测试：30 文件失败。其中 29 个是基线确认的既有/环境问题（含 `bundled-config-replacement` 的 EPERM rename，基线同样失败），1 个（`subagentRuntimeConfigWatcher`）是负载下偶发，单独复跑通过 |
| App 差分全量（`packages/services/tests/escode-cli-rust-*.test.ts`） | **400 个测试：391 通过 / 0 失败 / 9 跳过** |

**4.3 残留扫描**：`git ls-files | grep -i zcode` 只剩 `third-party/upstream-zcode/NOTICE.md`；文本扫描只剩保留项（主机名与转义写法、`zcode-plan`、`x-zcode-*`、`zcode-plugins-official`、`.zcode-plugin`）与 README 的 Apache-2.0 上游署名。

**与手册不同的判断（已复核后调整）**：

1. `dist-types/`：手册建议提交前 `git checkout --` 还原。实测还原会把 `@zcode/*` 写回已跟踪的 `.d.ts`（残留扫描立刻变红），而重建产物稳定复现同一内容（17 文件，±20 行，纯作用域改名与对应 sourcemap 位移），故保留提交。
2. 批量替换漏了带空格与带连字符的旧写法，本轮补齐：`Z Code`（退出确认弹窗文案、`X-Title` 客户端标识、架构文档标题）与 `z-code`（398 处 / 127 文件，全在注释、文档与夹具里，已确认无运行时 key 或路径依赖）。
3. `X-Title` 改为 `ESCode@…`：该值允许用户显式覆盖（见 `docs/superpowers/specs/2026-06-25-openrouter-attribution-headers.md`「不改变用户显式覆盖 X-Title 的语义」），因此不是服务端鉴权契约；Rust 与 Node 两侧同步改，App 差分里的 header 一致性用例通过。
4. remote runtime 的 CDN 路径前缀不一致（`remoteCdn.ts` 用 `/zcode/electron/releases`，CI/tsup 默认用 `escode/electron/releases`）属外部地址，未擅自改动，已记入 `docs/specs/escode-rebrand.md`「已知不一致（待用户确认）」。

