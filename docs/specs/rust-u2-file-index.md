# Rust 阶段 2：`file` 域的工作区文件索引与搜索（rust-unified-host.md 子 spec，优先序 4）

> 状态：已交付（主路径差分全等；极端 gitignore 写法见 §6.1）· 2026-10-15
> TS 真相源：`packages/services/src/file/{fileService.ts(索引/搜索段),workspaceFileIgnore.ts,workspaceFileMentionFilter.ts,workspaceFileSearch.ts}`、
> `packages/shared/src/{workspaceFileEntriesCodec.ts,workspaceFileSearch.ts}`。

## 1. 范围

`file` channel 中尚未实现的 6 个成员全部实现：`searchWorkspaceFiles`、`listWorkspaceFilesLength`、`listWorkspaceFilesRange`、
`readWorkspaceFileSearchIgnore`、`applyWorkspaceFileSearchIgnoreTransform`、`writeWorkspaceFileSearchIgnore`。
至此 `file` channel 17/17。

## 2. 所有者与状态

- Host 内唯一的 `FileIndexService`，进程内状态只有「索引缓存」：`workspaceKey → {at, signature, packed, candidates?}`
  （`workspaceKey = workspaceIdentity?.trim() || rootPath`；`signature = rootPath + "\n" + .escodeignore 指纹`）。
  - TTL 60 s；最多 4 项，LRU（命中即移到队尾，满则淘汰最旧）；过期项在每次 `ensure` 时清理。
  - 同 key 同 signature 的在途扫描被复用；`refresh=true` 绕过缓存与在途扫描，新扫描完成后替换（旧扫描只服务旧请求，不覆盖新索引）。
  - 指纹 = `.escodeignore` 的 `mtimeMs:size`，不存在为 `none`（Rust 取同源的 mtime 毫秒与长度；指纹字符串格式不外露，只用于变化检测）。
- 无连接级状态。规则文件的读写落在 workspace 自身目录，不属于 Host 状态。

## 3. 扫描与过滤（照搬 TS）

1. 加载 `.escodeignore` 规则：存在 → 用其内容；不存在 → 内存使用「`.gitignore` 拷贝 + 默认排除」模板（**不落盘**，TS 注释停用了自动创建）；
   读取失败 → 降级 `.gitignore` → 再降级内置默认模板（warn 一次）。规则语法是 gitignore（TS 用 `ignore` npm 包）。
2. 从 `rootPath` 起遍历目录（并发度只影响耗时，不影响结果）；`readdir` 失败且为 `EACCES/EPERM/ENOENT` 的目录跳过，其余错误整体失败。
3. 对每个条目：`relativePath`（posix 分隔）等于 `.escodeignore` 跳过；类型 = 目录 / 文件，软链接跟随目标（目标不可读按文件）；
   被忽略规则命中（目录用带尾 `/` 的路径匹配）跳过；再过默认过滤器（`ignoreRulesActive: true`）：
   目录 — 以 `.` 开头或位于隐藏目录内 → 不收录但继续遍历；文件 — `.env`、`.env.*`、`coverage.out`、`lcov.info` 与固定扩展名黑名单不收录。
   目录且非软链接且 `traverse` 才入队（软链接目录收录但不进入）。
4. 排序：目录在前，同类型按 `relativePath.localeCompare`（ICU，复用 `host-rpc/collation.rs` 注入的比较器）。
5. 打包：`type\t<转义 relativePath>` 用 `\n` 连接；转义 `\`→`\\`、TAB→`\t`、LF→`\n`。**长度以 UTF-16 码元计**（JS `string.length`）。

## 4. 方法

| 方法                                                                            | 行为                                                                                                                                                                                                                                  |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `searchWorkspaceFiles({rootPath, workspaceIdentity?, query, limit?, refresh?})` | `limit` 缺省 1000，`trunc` 后夹到 `[0,1000]`；`limit` 非有限数或 `query` 非字符串 → `Invalid workspace file search query or limit`；`limit=0` → `[]`；其余取索引（`refresh` 透传）→ top-K 打分 → 条目 `{name,path,relativePath,type}` |
| `listWorkspaceFilesLength({rootPath})`                                          | 索引（不传 identity、不 refresh）`packed` 的 UTF-16 长度，`Int`                                                                                                                                                                       |
| `listWorkspaceFilesRange({rootPath, offset, length})`                           | `packed` 的 UTF-16 区间 `[offset, offset+length)`（`offset`/`length` 先 `trunc`、夹到 ≥0；越界返回 `""`），裸字符串                                                                                                                   |
| `readWorkspaceFileSearchIgnore({rootPath})`                                     | 文件存在 → `{content, source:"file"}`；否则模板预览 `{content, source:"template"}`                                                                                                                                                    |
| `applyWorkspaceFileSearchIgnoreTransform({rootPath, transform})`                | `sync-gitignore` / `reset-defaults`，按两个分区标记重写；任一标记缺失则整体按模板重建                                                                                                                                                 |
| `writeWorkspaceFileSearchIgnore({rootPath, content})`                           | 原子写（同目录临时文件 + fsync + rename）                                                                                                                                                                                             |

打分（`getWorkspaceFileSearchCandidateScore`）逐字照搬：query/文本均 `trim().toLowerCase()`；`startsWith` → `len(text)-len(q)`；子串 → `100+index`；
否则子序列 → `200 + Σ间隔 + (len(text)-len(q))`；`name`、`relativePath+25`、`min(relativePath,path)+300` 取最小；字符串索引/长度全部按 UTF-16 码元。
top-K：按 `(score, 原索引)` 升序取前 `limit`；空 query → 目录排后、原序稳定。

## 5. 验收

1. **TS oracle 差分**：`packages/desktop/test/rustHostFileIndex.test.ts` 在同一组夹具目录（含 `.gitignore`/`.escodeignore` 的各种语法、隐藏目录、软链接、
   中文/空格/TAB/反斜杠文件名、node_modules、二进制扩展名、`.env*`）上，用真实 TS `createFileService()` 与 Rust Host 的 `file` channel
   对 6 个方法做 `toEqual`：搜索（空 query、前缀、子串、子序列、跨目录、中文、emoji、limit 边界、非法参数）、索引长度与分块拼接、规则读/转换/写（含标记缺失）。
2. **忽略规则语料差分**：同一批 `(规则文本, 路径, 类型)` 在 npm `ignore` 与 Rust 实现上逐条比较（取反、锚定、`**`、字符类、转义、尾空格、目录后缀、
   父目录排除后子文件不可恢复）。
3. **Rust 单测**：打分 UTF-16 语义、top-K 稳定性、打包转义往返、模板/分区转换、LRU 与 TTL。
4. **负向验证**：改坏打分（子序列间隔）、取反规则、打包转义、目录排序，差分必须失败。

## 6. 登记差异

1. **gitignore 实现**：TS 用 `ignore` npm 包（5.x，**默认大小写不敏感**——`readme.md` 会忽略 `README.md`，Rust 同样开启）；Rust 用 `ignore` crate。
   在 95 条规则 × 69 条路径（6460 个判定）的语料上，只剩 7 条规则与 npm 实现不同，都属极端写法，登记不修：
   | 规则写法 | npm `ignore` | Rust |
   | --- | --- | --- |
   | `\b`、`tab\<TAB>`（对普通字符转义） | 不当作转义，不匹配 | 按 git 规范当作字面字符 |
   | `[!a]*.md`、`[^a]*.md`（取反字符类） | 不按取反处理 | 按 git 规范取反 |
   | 单独的 `**/`、`/*/` | 不匹配根下文件 | 匹配得更宽 |
   | `docs/**/` | 连 `docs/` 本身也命中 | 只命中其下的目录 |
   主路径（通配、锚定、`**`、取反、目录后缀、字符类、尾空格、CRLF、注释、Unicode/emoji、父目录排除后子文件不可恢复）32 套规则的索引结果与打包内容全等。
2. **扫描遍历顺序**：并发遍历的发现顺序不影响结果（有全序排序）；`localeCompare` 判等但路径不同的条目相对顺序在两端都不保证。
3. **指纹格式**：不外露，仅变化检测。

## 7. 交付记录

| 项        | 证据                                                                                                                                                                                                                                                        |
| --------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 服务实现  | `host-services/src/file_index/`（`mod` 缓存与服务、`scan` 遍历与默认过滤、`ignore_rules` 规则与模板、`search` UTF-16 打分、`codec` 打包）                                                                                                                   |
| RPC 面    | `host-rpc/src/domains_file_index.rs` 并入 `file` channel；`host-proto` `IMPLEMENTED` 登记 6 个方法（`file` 17/17）                                                                                                                                          |
| Rust 单测 | 打分档位与 UTF-16、top-K 稳定性、打包往返（含 TAB/LF/反斜杠/Windows 根）、默认过滤/extname、缓存（TTL 内命中、refresh、规则变更失效）、长度与区间、规则预览/同步                                                                                            |
| TS 差分   | `packages/desktop/test/rustHostFileIndex.test.ts`：32 套规则 × 夹具树（索引内容与分块打包拼接）、30 个 query × 8 种 limit、非法参数、区间边界、规则读/转换/写（含标记缺失与无尾换行）。**两边必须用同一个根目录**——打分含绝对路径，先跑 TS、清空后再跑 Rust |
| 负向验证  | ① 子序列间隔加倍 ② 规则改成大小写敏感 ③ 打包不转义反斜杠 ④ 目录排序反转 ⑤ 同分按索引倒序，差分均失败                                                                                                                                                        |
