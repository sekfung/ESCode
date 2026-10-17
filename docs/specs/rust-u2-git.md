# Rust 阶段 2：`git` 域（rust-unified-host.md 子 spec，优先序 4）

> 状态：已交付（18/18；`generateCommitMessage` 见 §7）· 2026-10-16
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

| 项         | 约定                                                                                                       |
| ---------- | ---------------------------------------------------------------------------------------------------------- | ------ | ---------------------------------------------------------------- |
| 超时       | 缺省 15 s（diff 20 s、push 10 min）；到点 kill → 2 s 后强杀 → 再 2 s 仍未退出则按 orphaned 结束            |
| 输出上限   | stdout/stderr **各自**计字节（缺省 512 KiB、diff 1 MiB、push 8 MiB）；超限即 kill 并标记 `outputTruncated` |
| spawn 失败 | 不抛：`exitCode=-2`、`stderr=错误文案`，由上层 `ensureGitCommandSucceeded` 按失败处理                      |
| 错误文案   | `ensureGitCommandSucceeded` 的 `"<label> failed: <stderr                                                   | stdout | exitCode=…>"`/`timed out after …`/`output exceeded limit` 逐字同 |

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

| 项        | 证据                                                                                                                                                                                                                                                         |
| --------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 服务实现  | `host-services/src/git/`（`mod`/`command`/`paths`/`parse`/`parse_refs`/`files`/`reads`/`reads_diff`/`writes`/`writes_commit`）                                                                                                                               |
| RPC 面    | `host-rpc/src/domains_git.rs`；`host-proto` `IMPLEMENTED` 登记 17 个方法                                                                                                                                                                                     |
| Rust 单测 | `git/parse_tests.rs`（10 例）、`git/paths.rs` 内测试                                                                                                                                                                                                         |
| TS 差分   | `packages/desktop/test/rustHostGit.test.ts`：70 余步脚本，覆盖 §5.1 全部夹具；9 种分支 issue code 全部命中                                                                                                                                                   |
| 负向验证  | ① 分支排序当前分支不再优先 → `main branches` 失败；② void 方法回 `null` → `stage dup` 失败；③ 关掉 stagedOnly 临时 index 路径 → `commit staged only` 失败（初版夹具测不出：工作区与已暂存内容相同，`commit -- path` 结果一样；已加「暂存后再改工作区」一步） |

注：stagedOnly 提交后的 `reset --quiet HEAD -- paths` 对外不可见（提交的就是真实 index 里的条目，提交后它们已与新 HEAD
一致，reset 只刷新 stat 信息），因此无法做行为级负向验证；保留它是照搬 TS。

## 7. `generateCommitMessage`

TS 真相源：`gitService.ts`（输入采集）、`commitMessageFileScope.ts`、`gitCommitMessageGenerator.ts`、`node.ts` 的装配（当前模型 = ModelSelection 视图 `preferredSelection`，文本生成 = `agent.generateWorkspaceText`）。

流程与所有者：

```
git.generateCommitMessage(params)                       host-rpc/domains_git_commit_message.rs（编排）
  ├─ GitService::commit_message_inputs                  host-services/git/commit_message.rs
  │     status → 候选文件（includeUnstaged ? unstaged : []）+ staged → 会话作用域过滤 → 无文件则报
  │     "There are no changes available to commit." → 前 8 个文件的 diff（失败的丢弃，无 patch/summary 的丢弃）
  ├─ ProviderService::model_selection_view().preferredSelection   缺失 → GitCommitMessageGenerationError "未读取到当前模型。"
  ├─ build_prompt(...)                                  host-services/git/commit_message_prompt.rs（纯函数）
  ├─ runtime  workspace/generateText {workspace, selection, prompt, querySource:"git_commit_message"}（180 s）
  │     失败 → "模型请求失败。"（detail = 原因）
  └─ finalize_model_text                                空文本 → "模型请求失败。"/detail "模型响应缺少文本内容。"；
                                                        不合规 → "模型没有返回可用的 Conventional Commit 提交消息。"/detail=首行预览
返回 {message, providerId, model}
```

- **会话作用域**（`currentSessionFilePaths`）：绝对路径同时折算 repo 相对与 workspace 相对；相对路径在 monorepo 子目录 workspace 下补 `<workspaceInRepoPath>/` 前缀；文件按 `path/repoRelativePath/workspaceRelativePath` 任一命中；空来源 = 不过滤。
- **prompt 与校验逐字一致**由 TS 黄金样本守住：`scripts/generate-escode-host-commit-message-fixtures.mjs`（真实 `GitCommitMessageGenerator`，`--check` 已进 `check:escode-host`）导出 8 组 prompt（文件上限 20、diff 总量/单文件裁剪、会话上下文裁剪与换行归一、中文/英文、分支缺省）与 24 条输出校验（围栏/引号/`Commit message:` 前缀、100 码元上限、astral 字符按 UTF-16 计、`Revert` 大小写、CRLF）。JS 在码元边界截断会产生孤立代理项，样本刻意避开。
- **登记差异**：①错误的 `reason` 字段（`model-unavailable/request-failed/invalid-output`）不外露（RPC 只透传白名单键，`detail` 在其内）；②运行时 locale（`locale` 缺省时）取环境 `LC_ALL/LC_MESSAGES/LANG`，TS 取 `Intl` 解析的 locale；③TS 调用前同步账号 Provider 配置，Rust 暂无账号源；④会话作用域的 `relative()` 只实现 POSIX 语义（Windows 盘符路径未覆盖）。
- **验收**：`commit_message_prompt_tests.rs` 对黄金样本逐字比较；`rustHostCommitMessage.test.ts` 全链路——真实 Host 进程 + 内嵌 runtime + 本地假 OpenAI 兼容服务：成功、围栏包裹、不合规输出、会话作用域排空；同一端点同时验证了 `provider-settings.testModelConnectivity` 的成功与失败路径。
  负向验证：改 prompt 一句话 / 放宽 100 码元上限，黄金样本即失败。

## 8. `git-checkpoint` channel（4 个成员，2026-10-17）

TS 真相源：`packages/services/src/git/gitCheckpointService.ts`（服务层与错误语义）、
`git/repo/gitCheckpointRepo.ts`（git 管道）、`git/repo/gitCheckpointHelpers.ts`（解析/路径）、
`git/repo/gitCheckpointStore.ts`（manifest 存储）。§1 已注明「另立切片」，本节即该切片。

### 8.1 所有者与存储契约

manifest 由 Host 独占：Host 是唯一写者，Renderer 只经 RPC 间接读写。

| 项              | 约定                                                                                                                                                                            |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| manifest 根     | `<dataBaseDir>/.escode/v2/checkpoints`（TS `getAppConfigDir()`）                                                                                                                |
| manifest 文件   | `<根>/<workspaceHash>/<checkpointId>.json`，内容是 `JSON.stringify(meta, null, 2) + "\n"`                                                                                       |
| `workspaceHash` | `sha256(workspacePath)` 的 hex 前 12 位。**注意**：store 调 `getWorkspaceHash(workspacePath)` **不带 identity**，与 sessions 目录的 `workspaceKey` 口径不同——逐字复刻，不"统一" |
| 临时 index 根   | `<dataBaseDir>/.escode/git-checkpoint-index/index-XXXX/`（TS `getGitCheckpointIndexRootDir()`，**不在 `v2` 下**）；每次 `createCheckpoint` 一个目录，`finally` 删掉             |
| 原子写          | 同目录临时文件 + rename；TS 的临时名含 pid/时间/随机数（**不可观察**，Rust 用自己的方案）                                                                                       |
| 读校验          | TS `isGitCheckpointMeta` 浅校验（七个必填字段的类型 + `scope === "workspace"`）；读取/解析/校验任一失败一律回 `null`（不抛）                                                    |
| 写串行化        | TS 按**文件路径**串行（`writeChains`），避免同 id 并发写交错。Rust 侧写路径已经是同步 IO（`spawn_blocking_io` + 原子 rename），不需要第二套队列                                 |

### 8.2 行为（逐条对照 TS）

| 方法                                | 行为                                                                                |
| ----------------------------------- | ----------------------------------------------------------------------------------- |
| `createCheckpoint({workspacePath})` | `checkpointId = randomUUID()` → `repo.createCheckpoint` → `store.save` → 回 meta    |
| `diffCheckpoints`                   | 并发加载两个 manifest → `ensureComparableCheckpoints` → `repo.diffCheckpoints`      |
| `restoreBetweenCheckpoints`         | 同上 + `force`                                                                      |
| `deleteCheckpoint`                  | 加载 manifest → `repo.deleteCheckpoint`（删 hidden ref）→ `store.delete`（删 json） |

**服务层错误文案逐字**（都是 `Error`，无 `code`）：

- `Checkpoint does not exist: <checkpointId>`
- `Checkpoint workspace mismatch: <meta.checkpointId>`（meta 里的 id，不是入参）
- `Checkpoint repoRoot mismatch.` / `Checkpoint scope mismatch.` / `Checkpoint workspace scope mismatch.`
- `Git binary is not available in the current environment.` / `Workspace is not inside a Git repository.`
- `Checkpoint restore verification failed.`

**`createCheckpoint` 的 git 管道**（顺序即语义）：

1. `resolveRepository` → 前置校验（上面两条错误）；
2. `refName = refs/escode/checkpoints/<workspaceHash>/<checkpointId>`（同一份 hash 口径）；
3. 临时 index **预热**（性能路径，语义等价）：优先 `rev-parse --git-path index` 后复制用户真实 index（复用 stat cache）；失败则 `read-tree HEAD`（带临时 env）；再失败留空 index；
4. `add -A -- <pathspec>`（`GIT_INDEX_FILE` + 四个 `ESCode Checkpoint` 身份变量）→ `write-tree` → `commit-tree <tree> -m "escode checkpoint <checkpointId>"` → `update-ref <refName> <commitOid>`，四步任一失败即报错；
5. `finally` 删临时 index 目录。**不碰用户真实 index、不产生用户可见分支**（这是本域的核心不变量）。

**`diff`**：`diff --name-status --find-renames -z <from> <to> -- <pathspec>` 与
`diff --numstat --find-renames -z ...` 两路，合并成 `{fromCheckpointId, toCheckpointId, files[]}`；
`files[]` 每项的 `path`/`originalPath` 是**绝对路径**，`workspaceRelativePath` 由 repo 相对路径折算。

**`restore`（三方语义，本域最核心）**：

1. 算 `from → to` 的受影响路径集（含 rename 的旧路径），归一化成 repo 相对路径；
2. 集合为空 → `{success: true, restoredPaths: []}`（**不查冲突、不报错**）；
3. `collectWorkspaceConflicts(from)`：对受影响路径逐个判定——`ls-tree -r -z <fromOid>` 里没有该条但磁盘存在 → `unexpected-file-in-worktree`；磁盘不存在 → `missing-in-worktree`；磁盘是目录（或期待符号链接却不是）→ `type-mismatch`；`hash-object --no-filters` 与 tree 条目的 objectId 不等 → `content-mismatch`。**按 blob hash 判定，不比时间戳/大小**；结果按 `repoRelativePath` 去重；
4. 有冲突且 `force !== true` → `{success: false, conflicts}`，**不写盘**；
5. `restore --source=<to.commitOid> --worktree -- <非 deleted 的 repo 相对路径>`（只改 worktree，**不动真实 index**）；
6. 删残留：`deleted` 的绝对路径 + `renamed` 的 `originalPath`（避免新旧并存）；
7. 以 `to` 为基线再查一次冲突，**仍有冲突即抛 `Checkpoint restore verification failed.`**（不把错误状态固化成 success）；
8. 回 `{success: true, restoredPaths: files.map(f => f.path)}`（绝对路径、含 deleted）。

**`delete`**：`update-ref -d <refName>`，退出码 `[0, 1]` 都算成功（ref 不存在不是错误）。

### 8.3 登记差异

1. **`createdAt` 用 Host 注入时钟**（TS `Date.now()`）。CLI 侧没有跨进程时钟注入，故同 u1 的确定性约定：测试用注入时钟，生产取系统时间。
2. **`randomUUID`**：`checkpointId` 由 Host 生成（TS `node:crypto.randomUUID`），Rust 用 `uuid` v4——两者都不可预测、格式相同，值不逐字相同（不可观察：id 只由调用方回传）。
3. **临时 index 目录名**不可观察（TS `mkdtemp` 前缀 `index-`，Rust 同前缀）。
4. **`collectWorkspaceConflicts` 的 `ls-tree` 是单次批量调用**（TS 同），但 TS 之后对每个路径**串行** `hash-object`；Rust 保持串行（并发会改变 git 进程数，且此处不是热路径）。
5. TS 的 `writeChains` 串行化在 Rust 侧不需要（见 §8.1），属实现差异、不可观察。

### 8.4 验收

- **解析器单测**（`checkpoint_parse_tests.rs`）：`--name-status -z` 的 A/M/D/R/C 与 rename 三段式、畸形记录跳过；`--numstat -z` 的 `-` 计数、rename 的 path 空字段回退、同 rename 写新旧两条；`ls-tree -z` 的 mode/type/objectId 与缺 tab 跳过。
- **store 单测**：`workspaceHash` 口径（**不带 identity**）、落盘格式（2 空格缩进 + 结尾换行）、`load` 对缺字段/错 scope/坏 JSON 一律 `null`、`delete` 幂等。
- **端到端**（`rpc_git_checkpoint.rs`，真临时 git 仓库 + 真 WebSocket）：建库 → 改文件 → `createCheckpoint` → 再改 → `createCheckpoint` → `diffCheckpoints`（断言 kind/added/removed 与绝对/相对路径三种写法）→ 手工改脏其中一个受影响文件 → `restoreBetweenCheckpoints` 回 `{success:false, conflicts:[content-mismatch]}` 且**磁盘未被写**（负向可观察）→ `force:true` 恢复成功 → 断言磁盘内容回到 to 基线 → `deleteCheckpoint` 后 `diffCheckpoints` 报 `Checkpoint does not exist`。
- **不变量断言**：整个流程后 `git status --porcelain` 的 staged 区为空（证明没有污染用户 index）、`git branch --list` 无新分支、`git for-each-ref refs/escode/checkpoints` 只在 create 后出现、delete 后消失。
- **负向验证**：①冲突判定改成比大小/时间戳 → 冲突用例不再命中（或误命中）；②`restore` 的 `--worktree` 去掉 → staged 区不变量失败；③`refName` 的 hash 换成带 identity 的口径 → 与 TS 的 ref 名不匹配（由 store/ref 两个单测钉住）。

### 8.5 交付记录（2026-10-17）

- **服务层**：`host-services/src/git/{checkpoint,checkpoint_conflicts,checkpoint_parse,checkpoint_store,checkpoint_service}.rs`
  （各自 ≤400 行；`GitService::git` 放宽为 `pub(crate)` 供同域子模块复用，**共用同一实例**——组合根
  `domains_git::git_service()` 建一次，`git` 与 `git-checkpoint` 两个通道共享）。
- **RPC 面**：`host-rpc/src/domains_git_checkpoint.rs`；缺 `dataBaseDir` 时方法表照常登记、调用回结构化缺口
  （契约对账要求 `Implemented` 必须能在表里找到）。`host-proto` `IMPLEMENTED` 登记 4 个方法。
- **测试**：`checkpoint_store_tests.rs`（4 例：hash 口径、落盘字节格式、读校验 fail-closed、删除幂等）、
  `checkpoint_parse_tests.rs`（4 例）、`tests/rpc_git_checkpoint.rs`（主流程，真 git 仓库）、
  `tests/rpc_git_checkpoint_lifecycle.rs`（删除与错误文案）。
- **负向验证**（两项，均已恢复）：
  1. 冲突检测恒回空 → 主流程用例在「被冲突挡住」处失败（restore 直接覆盖了脏文件，`success:true`）；
  2. `restore --worktree` 改成 `--staged` → 复核步骤抛 `Checkpoint restore verification failed.`，用例失败。
- **实现期发现并修正的错误**：`restoreBetweenCheckpoints` 的**方向**——`from` 是调用方**声明磁盘当前所在**的
  状态、`to` 才是目标（「撤销 A→B 之间的工作」是 `from=B, to=A`，不是反过来）。首版用例写反了方向，
  结果是真实的冲突（`b.txt` 不在 A 的 tree 里却在盘上 → `unexpected-file-in-worktree`）被误当成实现缺陷。
  该方向已写进 §8.2 的对照表。
