# Rust 阶段 2：`git` 域（rust-unified-host.md 子 spec，优先序 4）

> 状态：已交付（17/18，`generateCommitMessage` 登记缺口）· 2026-10-15
> TS 真相源：`packages/services/src/git/{gitService.ts,config.ts}`、`git/repo/{gitCliRepo.ts,gitCliHelpers.ts,gitCliTypes.ts}`、
> `git/providers/{gitCommandProvider.ts,gitEnvironmentProvider.ts}`。`git-checkpoint` channel 另立切片。

## 1. 范围

`ServiceChannels.Git = "git"` 的 18 个方法中实现 17 个：`getRepositorySummary`、`getWorkspaceRepositoryInfo`、
`getLocalBranches`、`getCommitGraph`、`switchBranch`、`createBranchAndSwitch`、`getChanges`、`getIgnoredPaths`、`getDiff`、
`getBranchComparison`、`stagePaths`、`unstagePaths`、`discardPaths`、`commit`、`push`、`getIdentity`、`refresh`。

不做：`generateCommitMessage`（依赖模型调用的 commit message 生成器 + 会话文件作用域，随 providers/模型调用面迁移；
期间回显式缺口）。

## 2. 所有者与状态

- **Git CLI 是唯一事实源**：服务不缓存仓库状态，每次调用重新执行 `git`（结果与 TS 相同；TS 的 in-flight 去重只是
  并发合并，不改变结果，Rust 不复制）。
- **唯一的持久进程态**：`collapsedUntrackedRepoRoots`——某 repoRoot 的详细 `status` 一旦超出 512 KiB 输出上限，
  此后该 repoRoot 一律用 `--untracked-files=normal`（目录折叠）。它改变返回结果，必须照搬；Host 内唯一实例。
- **git 二进制**：候选 `ESCODE_GIT_BINARY` → `git` →（Windows）Program Files 四个位置，首个 `--version` 成功者，进程内缓存。
- **命令 env**：Host 进程 env（经 `core_api::env` 唯一出口）剔除 16 个 `GIT_*` 本地变量，再叠加
  `GIT_OPTIONAL_LOCKS=0`、`GIT_PAGER=cat`、`PAGER=cat`、`TERM=dumb`、`LC_ALL=C`、`LANG=C`（防 pre-push hook 注入的
  `GIT_DIR` 让命令串仓）。

## 3. 命令执行语义（TS `gitCommandProvider`）

| 项 | 约定 |
| --- | --- |
| 超时 | 缺省 15 s（diff 20 s、push 10 min）；到点 kill → 2 s 后强杀 → 再 2 s 仍未退出则按 orphaned 结束 |
| 输出上限 | stdout/stderr **各自**计字节（缺省 512 KiB、diff 1 MiB、push 8 MiB）；超限即 kill 并标记 `outputTruncated` |
| spawn 失败 | 不抛：`exitCode=-2`、`stderr=错误文案`，由上层 `ensureGitCommandSucceeded` 按失败处理 |
| 错误文案 | `ensureGitCommandSucceeded` 的 `"<label> failed: <stderr|stdout|exitCode=…>"` / `timed out after …` / `output exceeded limit` 逐字同 |

## 4. 接口与形状

- 返回对象字段与 TS 相同；TS 中值为 `undefined` 的字段整键省略（如 `GitFileChange.x/y` 为 null 时）。
- 路径：`path.resolve` / `path.relative`（POSIX 语义；Windows 盘符与反斜杠按 Node `win32` 规则）、
  `normalizeInputPath` 的 `realpath` 失败回退原路径。
- 排序：`getLocalBranches` 当前分支优先 → 提交时间降序 → `localeCompare`（复用 `host-rpc/collation.rs` 的 ICU 排序）。
- numstat：保持 `Map` 插入序（重复键覆盖值、位置不变）。

## 5. 验收

1. **TS oracle 差分**（最高优先级）：`packages/desktop/test/rustHostGit.test.ts` 在同一组临时仓库夹具上分别调用真实 TS
   `createGitService()` 与 Rust Host 的 `git` channel（真实 `@escode/client`），`toEqual` 全等。夹具覆盖：非仓库目录、
   空仓库、子目录 workspace（monorepo 作用域）、staged/unstaged/untracked/冲突/重命名/删除、二进制文件、upstream 分支对比
   （ahead/behind）、分支切换失败归类（覆盖本地改动）、commit（全部与 stagedOnly 两种）、identity。
2. **Rust 单测**：porcelain v2 / numstat / graph / branch refs / config value / 分支错误归类的解析边界。
3. **负向验证**：翻转一条过滤（如去掉 untracked 的 0 行过滤）或排序规则，差分必须失败。

## 6. 登记差异

1. **`generateCommitMessage` 缺口**：见 §1。
2. **输出解码**：TS 按管道块逐块 `toString("utf-8")`，多字节字符恰好跨块时会产生 U+FFFD；Rust 整体解码。只在该边界可见，
   Rust 结果更正确。
3. **in-flight 去重不复制**：并发同一 workspace 的调用会各自执行 git（结果相同，git 调用次数更多）。
4. **超时杀进程**：TS 先 `child.kill()`（SIGTERM），2 s 后强杀（Windows `taskkill /T /F` 杀进程树）；Rust 到点直接
   `start_kill`（SIGKILL / `TerminateProcess`，不含进程树）。两者都在 kill 后再等宽限期，`close` 未到即按 orphaned 结算
   （放弃管道，不无界等待持有管道的 hook 孙进程）。结算结果（`timedOut`、错误文案）一致；只影响被杀进程能否做清理。

`generateCommitMessage` 不实现的原因（§1）：TS 组合根（`packages/services/src/node.ts` `GitCommitMessageGenerator`）依赖
① `providerRuntime.modelSelection` 读用户首选模型（providers 域，未迁移）与 ② `generateWorkspaceText` 经运行时生成文本；
而嵌入 Rust Host 的运行时目前缺 provider 配置 env（审计 A16），只能用 `--config` 模型。现在实现只会失败或静默换模型，
不如显式缺口；随 providers 域 + A16 一起交付（文件作用域过滤与 diff 收集是纯计算，届时直接平移）。

## 7. 交付记录

| 项 | 证据 |
| --- | --- |
| 服务实现 | `host-services/src/git/`（`mod`/`command`/`paths`/`parse`/`parse_refs`/`files`/`reads`/`reads_diff`/`writes`/`writes_commit`） |
| RPC 面 | `host-rpc/src/domains_git.rs`；`host-proto` `IMPLEMENTED` 登记 17 个方法 |
| Rust 单测 | `git/parse_tests.rs`（10 例）、`git/paths.rs` 内测试 |
| TS 差分 | `packages/desktop/test/rustHostGit.test.ts`：70 余步脚本，覆盖 §5.1 全部夹具；9 种分支 issue code 全部命中 |
| 负向验证 | ① 分支排序当前分支不再优先 → `main branches` 失败；② void 方法回 `null` → `stage dup` 失败；③ 关掉 stagedOnly 临时 index 路径 → `commit staged only` 失败（初版夹具测不出：工作区与已暂存内容相同，`commit -- path` 结果一样；已加「暂存后再改工作区」一步） |

注：stagedOnly 提交后的 `reset --quiet HEAD -- paths` 对外不可见（提交的就是真实 index 里的条目，提交后它们已与新 HEAD
一致，reset 只刷新 stat 信息），因此无法做行为级负向验证；保留它是照搬 TS。
