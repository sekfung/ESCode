# Rust 阶段 2：sync 三域（rust-unified-host.md 子 spec）

> 状态：实现中 · 2026-10-17。三个域共 19 个成员：
> `skill-sync` 5（已交付）、`mcp-sync` 8、`settings-sync` 6。按域分三次提交。
> 共同点：都是 **Host 级服务**（TS 在 local 与 remote 两侧都注册），用户根是 `rpcListen.home`
> （homedir 语义，经 `channels_with(home, …)` 注入），不依赖工作区或 agent 实例。
> 远端同步的方向：本地 Host 导出 → 文件/网络传输 → 远端 Host 导入；归档与配置文件是两端共享的事实。

## 1. skill-sync（5/5）

TS 真相源：`skill-sync/skillSyncService.ts` 509、`skillSyncArchive.ts` 346、`skillSyncPath.ts` 43、
`skillSyncErrors.ts` 23、`skillSync.ts` 30。

### 1.1 接口

| 方法                                      | 行为                                                                         |
| ----------------------------------------- | ---------------------------------------------------------------------------- |
| `listLocalUserSkillCandidates()`          | 扫 `.escode/skills` + `.agents/skills`，返回候选与 `maxArchiveBytes`（20 MiB） |
| `listRemoteUserSkillStatuses({…})`        | 按目录名判断远端是否已存在；支持按 skill `name` 回退匹配已有目录             |
| `exportSkillsArchive({skillIds})`         | 按候选 id 选目录，生成 gzip+tar 归档；字节数上限 20 MiB                      |
| `checkRemoteUserSkillWriteAccess()`       | 在 `.escode/skills` 写入并删除探测文件，返回 `{ok,path,error?}`              |
| `importSkillsArchive({archive,overwrite})` | 解压到临时目录，按目录名与 skill 名双重判重后复制；返回逐项结果              |

错误形状：普通错误 `name = "Error"`；超限错误 `name = "SkillSyncSizeLimitError"`、
`code = "SKILL_SYNC_SIZE_LIMIT_EXCEEDED"`、`data = {actualBytes,maxBytes,phase}`（UI 按 phase 本地化）。

### 1.2 候选扫描

- 复用 `skills` 域的有界遍历策略（`should_walk`、`MAX_SCAN_DEPTH`、`SKILL.md`），
  不另写递归；扫描错误静默跳过（TS 不传 `onError`）。
- 每个 `SKILL.md` 的目录成为一个候选：
  - `directoryName` = 相对根的路径（`/` 分隔）；根目录自身的 SKILL.md 不算候选。
  - `name` = frontmatter `name.trim()`，缺失时回退为**相对路径最后一段**。
  - `description` = frontmatter `description.trim()`，缺失为空串。
  - `id` = `sha256(绝对目录路径)` 的十六进制（64 字符）。
  - `path` = `SKILL.md` 绝对路径。
  - `sizeBytes` = 递归体积：软链跟随目标（文件取目标 size、目录按目标递归），
    目录按 realpath 去重（环返回 0）；不跳过 `node_modules` 等目录（体积是真实占用）。
- **去重顺序**：
  1. `.escode` 候选按 `directoryName` 排序后，按 `SKILL.md` 的 canonical path 去重（realpath 失败回退原路径）；
  2. `.agents` 候选先排除两类：目录名属于 `.escode` 下**任何**含 `SKILL.md` 的目录（含非候选路径），
     frontmatter name（trim+lowercase）与 `.escode` 候选重名；
  3. 再与 `.escode` 共用 canonical path 去重；
  4. 合并后按 `directoryName` 以 Node `localeCompare`（复用 `collation::compare_names`）排序。

### 1.3 远端状态

- 根固定为 `.escode/skills`；`directoryNames` 逐个做安全相对路径归一（拒绝绝对路径、`..`、
  反斜杠、盘符、空段）。
- 存在（`existsSync`）→ `{directoryName, exists:true, path}`。
- 不存在时，若请求的 `skills` 里有同名条目，用其 `name` 在**用户级两个根**（`.escode` 先、`.agents` 后）
  按 skill 名找已有目录；找到 → `exists:true` + 已有路径；否则 `exists:false`。
- skill 名键 = `trim().toLowerCase()`；同名以遍历中先出现者为准。

### 1.4 归档格式

- **gzip + ustar tar**，只写目录（type `5`，路径带尾 `/`）与普通文件（type `0`）：
  - 目录内容按文件名 `localeCompare` 排序后递归；符号链接/特殊文件 → `unsupported skill archive source: …`。
  - 头字段与 TS 相同：mode（`st_mode & 0o777`）、uid/gid 0、size、mtime 秒、`ustar`/`00`、
    uname/gname `escode`；路径 >100 字节时按段拆 `prefix`（≤155）+ `name`（≤100），否则报路径过长。
  - 末尾 1024 字节全零块。
- **读取**：流式解压（不一次性展开，避免压缩炸弹占内存）；只接受 type `0`/`\0`/`5`；
  文件累计字节超过 `maxExtractedBytes` → `extracted-content` 超限；路径逐个做安全归一并限制在目标根内；
  截断 → `truncated skill sync archive`。
- 上限 20 MiB 在三个阶段分别校验：`selected-content`（候选体积和）、`archive`（gzip 字节数）、
  `extracted-content`（解压文件累计）。

### 1.5 导入语义

- `overwrite` 为真 → `Error("skill sync overwrite is not supported")`。
- 先解压到 `<root>/.sync-tmp-<uuid>`，结束后无论如何删除临时目录。
- 提取目录按 `directoryName` 排序；逐项：
  1. 目标已存在 → `skipped` + 目标路径；
  2. 已有同名 skill（用户级两个根）→ `skipped` + 已有路径；
  3. 否则递归复制（文件 `create_new`，已存在即失败；目录合并；软链按链接复制）→
     `synced` + 目标路径，并把名字写入本次的已存在表；
  4. 复制失败 → `failed` + `error`。
- `SKILL.md` 缺失的项（收集后竞态）→ `failed` + `error = "SKILL.md is missing"`。

### 1.6 写权限探测

- `mkdir -p <root>` 后以独占创建写 `.escode-sync-preflight-<pid>-<uuid>`（内容 `ok`）再删除。
- 失败返回 `{ok:false,path,error}`，不抛错；清理失败忽略。

### 1.7 登记差异

1. 目录遍历的并列顺序：`.escode` 内同名 skill 的「先出现者」按路径有序遍历（Rust BTreeSet）
   vs TS 的 DFS 产出顺序；只在同一根内出现重名时可见。
2. 错误文案：文件系统错误用 Rust `io::Error` 文本（TS 是 Node 的 `code: message, syscall 'path'`）；
   稳定 `code`（超限）不受影响。
3. gzip 头（OS/压缩实现）字节不同；tar 负载与 TS 读取器互操作。

### 1.8 验收

- 单测：路径归一/越界拒绝；归档往返（目录+文件、排序、mode/mtime、路径拆分）、恶意路径拒绝、
  截断与不支持类型、解压上限；候选扫描（两级根、目录名/名字去重、realpath 去重、嵌套 skill、
  体积与软链）、远端状态（直接存在、按名字回退、不存在）；导入（synced/skipped/失败、临时目录清理）；
  写权限探测。
- 端到端：临时 home 起服务，`listLocalUserSkillCandidates` 返回夹具；A 导出 → B 导入 → B 再扫到
  同名候选，二次导入全部 skipped；`checkRemoteUserSkillWriteAccess` 为 ok。
- 负向验证：①去掉 `.agents` 的名字过滤 → 重名用例失败；②去掉解压路径归一 → 穿越归档用例失败。

### 1.9 skill-sync 交付记录（2026-10-17）

- **实现**：`host-services/src/skill_sync/`
  - `candidates.rs`：两级根扫描（复用 `skills` 域的 `walk_markdown_paths` / `policy`）、
    `.escode` 目录名与 skill 名双重过滤、realpath 去重、递归体积（软链跟随 + 环去重）、
    TS 口径的 frontmatter 解析（saphyr，失败回退目录名）；
  - `paths.rs`：安全相对路径归一与根内解析；
  - `archive.rs` + `archive_tar.rs`：gzip + 手写 ustar（type `0`/`5`、mode/mtime/prefix/checksum），
    流式解压、`parseInt(_, 8)` 口径的八进制解析、三阶段上限；
  - `import.rs`：临时目录解压 → 目录名/name 双重判重 → `synced/skipped/failed`；
  - `service.rs`：五个成员 + 写权限探测。
- **通道**：`domains_skill_sync.rs`（`skill-sync`，Host 级，home 经 `channels_with` 注入）；
  `inventory_services.rs` 补 5 条。
- **共享件**：`host-rpc/src/wire_bytes.rs` 集中 `@escode/rpc` 的嵌套 Uint8Array 编解码，
  `serial.write` 的 `param_bytes` 改为复用同一实现；`skills` 域把遍历与扫描策略提为 crate 可见。
- **测试**：新增 17 例（路径 2、tar 头 3、归档 5、候选 3、服务 5——含导入/跳过/三阶段超限/写权限）
  + 端到端 1 例（A 导出 → B 导入 → 二次 skipped；未知 id 与不安全路径的错误形状）。
- **负向验证**（两项，均已恢复）：①去掉 `.agents` 的 skill 名过滤 → 去重用例失败；
  ②去掉解压路径归一与根内限制 → 穿越归档用例失败（并确认文件真的落到了目标根之外）。
- **门禁**：workspace 全部测试目标通过（host-services 434、host-rpc 119 及全部 e2e）；
  clippy `-D warnings` 0；`pnpm check:escode-host` 通过；库存 **278 已实现 / 260 待办**。
