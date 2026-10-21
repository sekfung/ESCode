# Rust 阶段 2：`gen-ui` 域（rust-unified-host.md 子 spec）

> 状态：已交付（5/6；`prepareSandbox` 按设计不在本域）· 2026-10-17。范围界定与插件 UI 沙箱的判定见 `rust-u2-host-scope-boundary.md` §3.2。
> TS 真相源：`packages/services/src/gen-ui/adapters/{service,files,stateStorage}.ts`、
> `packages/shared/src/gen-ui/contract.ts`、`packages/shared/src/node/genUiPaths.ts`。

## 1. 范围

**做**（5 个成员，全部是 Host 本地能力，不经运行时）：

| 成员             | 职责                                                                             |
| ---------------- | -------------------------------------------------------------------------------- |
| `readDocument`   | 受限读取模型写出的 HTML 片段（输出根前缀 + 逐级拒符号链接 + 打开后核对文件身份） |
| `getState`       | 读某 widget 的状态，并**建立该文件的通知基线**                                   |
| `setState`       | 原子写状态 + 容量淘汰 + 通知                                                     |
| `listState`      | 列某 scope 下全部状态（按 `updatedAt` 降序）                                     |
| `onStateChanged` | 状态变更事件（进程级广播）                                                       |

**不做**：`prepareSandbox`——它的生效步骤在 Electron Main（`getPluginSandboxHost().registerFromHost`，
返回的句柄带 `shellUrl`/`partition`），Rust Host 无 webContents/partition，判定与依据见边界子 spec §3.2。

## 2. 所有者与路径

**唯一所有者**：Host 进程内一份状态存储（`GenUiStateStore`）。Renderer 只经 RPC 读写；
**模型不参与**——文件由模型写入输出目录，Host 只读。

| 项         | 约定                                                                                                                                                      |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 状态根     | `<dataBaseDir>/.escode/v2/gen-ui-state`（TS `getAppConfigDir()/gen-ui-state`）                                                                            |
| scope 目录 | `<状态根>/<sha256(buildGenUiScopeKey(scope))>`                                                                                                            |
| 状态文件   | `<scope 目录>/<sha256(target.path)>.json`（**两个 hash 都是完整 64 位十六进制**，不截断）                                                                 |
| 输出根     | `<dataBaseDir>/.escode/v2/visualizations`（TS `getAppConfigDir()/GEN_UI_OUTPUT_DIRECTORY`；Main spawn agent 时经 `ESCODE_GEN_UI_OUTPUT_ROOT` 注入同一值） |
| 文件上限   | 记录 ≤ 96 KiB；widget state ≤ 16 KiB（序列化后 UTF-8 字节）；HTML ≤ 5 MB                                                                                  |
| 容量       | 每个 scope 至多 100 条记录                                                                                                                                |

`buildGenUiScopeKey(scope)` = `JSON.stringify([identity?.trim() || workspacePath, sessionId])`——
**数组**（键序固定），且 JS 字符串转义必须逐字一致（它是 hash 输入，差一个字节就换目录）。

## 3. 接口与语义

### 3.1 `readDocument(target) → {path, html}`

校验顺序即安全边界（TS `readGenUiDocument`，逐条复刻）：

1. **schema**：`target` 与 `path` 见 §4；`outputRoot`/`path`/`workspacePath` 必须都是绝对路径，否则
   `Invalid executor path`；
2. **前缀**：`relative(root, normalize(path))` 为空、为 `..`、以 `../` 开头或是绝对路径 → `Gen UI file is outside the host output directory`
   （注意：**不限** scope 子目录，读取范围只由输出根决定——fork 会继承父会话路径）；
3. **逐级拒链**：`realpath(root)` 后按 `/` 分段拼接，每段 `lstat` 是符号链接即
   `Gen UI does not read symbolic links`（realpath 后再判前缀会放行替换中的链接，故逐级判）；
4. **再看前缀**：`relative(canonicalRoot, realpath(current))` 仍须在根内，否则 `Gen UI file escaped the output directory`；
5. **常规文件**：`lstat` 非文件或 size > 5 MB → `Gen UI requires a regular file under 5 MB`；
6. **打开后核对身份**：`O_RDONLY | O_NOFOLLOW` 打开，`fstat` 必须是文件且 `dev`/`ino` 与打开前一致，否则
   `Gen UI file changed while opening`；
7. **读**：最多读 5 MB + 1 字节，超出即 `Gen UI file exceeds 5 MB`；UTF-8 **严格**解码（非法字节即错）；
8. **片段校验**（`assertGenUiFragment`）：> 5 MB → `Gen UI file exceeds 5 MB`；
   命中 `<(?:!doctype\b|\/?(?:html|head|body)(?:\s|>))`（不区分大小写）→ `Gen UI requires an HTML fragment`。

### 3.2 `getState` / `setState` / `listState`

- **`observe(target)`**：`mkdir -p` scope 目录 → 登记 `file → target` → 该目录**首次**观察时装一个目录
  watcher（只看文件名匹配 `^[a-f0-9]{64}\.json$` 的事件）。
- **`getState`**：observe → 读记录 → 回 `state`，**首个读取者建立通知基线**
  （`lastPublished[file] = JSON.stringify(state)`），否则一次读取会把别的页面等着的变更"吃掉"。
- **`setState`**：schema 校验 → 按 **scope key 串行**（同一 scope 的读-改-写不能交错）→
  取 `<scope 目录>/state` 的**跨进程文件锁**（`[25,50,100,200,400]ms`、ownerless grace 100ms、上限 8s）
  → 读现有记录 → `updatedAt = max(now, max(现有 updatedAt) + 1)`（同毫秒连写也要单调）→ 原子写 →
  **淘汰**：除刚写的文件外，按 `updatedAt` 降序保留 99 条、其余删除 → 释放锁 → publish。
- **`listState(scope)`**：读目录下文件名合法的记录、**按 scope key 过滤**（记录里的 target 必须同 scope）、
  按 `updatedAt` 降序、取前 100、投影成 `{path, state}`。

### 3.3 `onStateChanged`（`{target, state}`，`state` 可为 `null`）

- **通知口径**：读该文件当前记录（读不到即 `null`）→ 与 `lastPublished[file]` 比较 →
  **相同则不发**（去重，避免 watcher 与写入 ACK 互相回声）→ 否则更新基线并广播。
- **每文件串行**：watcher 事件与写入 ACK 可能同时到；串行读保证较早的快照不会迟到覆盖较新的通知。
- 事件是**进程级广播**：每个订阅者一条独立流，无 key、无初始快照。

## 4. schema（逐条对照 zod）

`genUiPathSchema`：长度 1..4096（**UTF-16 码元**）；不含 `NUL`/`CR`/`LF`；按 `[\\/]` 分段的任一段不得为 `..`；
以 `/`、`[A-Za-z]:[\\/]` 或 `\\` 开头；**末段**匹配 `^[a-z0-9]+(?:-[a-z0-9]+)*\.html$`。

`genUiScopeSchema`：**strict**，`workspacePath` 1..4096、`workspaceIdentity?` ≤4096、`sessionId` 1..512。
`genUiStateTargetSchema` = scope + `path`。任何未声明键即失败。

`genUiWidgetStateSchema`：**strict** 的 `{modelContent, privateContent}`，两者缺省为 `null`（`.default(null)`）；
随后序列化并检查 UTF-8 字节 ≤ 16 KiB。TS 的 reviver 还会拒绝 `undefined`/函数/`symbol`/`bigint`/非有限数——
这些都不是 JSON 值，经 RPC 不可能到达 Host，Rust 侧只保留「strict 键 + 缺省 null + 16 KiB」三条。

记录 schema：**strict** 的 `{target, state, updatedAt: number}`；文件 > 96 KiB 直接判无效。

## 5. 登记差异

1. **JSON 键序**：`serde_json` 未启用 `preserve_order`，`Value` 对象按键名字典序输出；TS 保持插入序。
   因此 Rust 写的记录文件与 TS 写的**字节不同**（键序不同），但两边都用 strict schema 互读、字段集合相同，
   **互操作无损**。可见条件：直接 diff 两个 Host 写的同一份记录文件。
2. **`fs.watch` 的 `persistent: false`**（不让进程因此存活）在 tokio 下无对应面：watcher 是 Host 内的
   任务句柄，Host 退出即随之消失。
3. **文件名过滤依赖 watcher 提供的文件名**：TS 拿 `fs.watch` 的 `filename`；`notify` 给的是路径，
   Rust 取**路径末段**同样过滤 `^[a-f0-9]{64}\.json$`（TS 在拿不到 filename 时直接跳过，Rust 不会遇到该分支）。
4. **`dispose()`**：TS 服务在 `createLocalServices` 收尾时释放 watcher；Rust 的 watcher 随 Host 进程生命周期，
   不额外暴露释放入口（对应 `domains_simple` 的既有形态）。
5. **错误文案**与 TS 逐字一致（`Error`，无 `code`）；`setState` 的文件锁超时沿用 `escode-cli-host::file_lock`
   的错误形状（TS 是 `ESCODE_FILE_LOCK_TIMEOUT`）。

## 6. 验收

- **schema 单测**：路径 schema 的八个分支（相对路径、`..` 段、大写/非法末段、含 `NUL`/换行、超长、
  非 `.html`、合法三种前缀）；scope/target 的 strict 拒绝未知键；widget state 的缺省 null、strict、16 KiB 边界（恰好 16 KiB 允许）。
- **文档读取单测**（真临时目录）：正常读回；根外路径拒绝；中间段符号链接拒绝；**末段**符号链接拒绝；
  目录/非文件拒绝；> 5 MB 拒绝（含恰好 5 MB 允许）；非法 UTF-8 拒绝；含 `<!doctype html>`/`<body>` 的完整文档拒绝、
  纯片段允许；非绝对路径拒绝。
- **状态存储单测**：`updatedAt` 单调（同毫秒连写 +1）；容量淘汰（写 101 条后剩 100 且最新的在）；
  `listState` 的 scope 过滤与降序；记录 > 96 KiB 判无效；坏 JSON 记录使 `listState` **整体失败**
  （TS 的 `Promise.all` 同——不静默跳过；只有 ENOENT 算"没有这条"）；
  通知去重（同值不发）与 `state: null`（记录被删）。
- **端到端**（`rpc_gen_ui.rs`，真 WebSocket）：`setState` → `getState` 读回 → `listState` 含该条 →
  订阅 `onStateChanged` 后 `setState` 收到事件、**同值再写不重复发**；模型侧直接改文件（模拟模型写文件）后
  watcher 触发通知；`readDocument` 读回模型写的片段并拒绝输出根外的路径。
- **负向验证**：①去掉逐级符号链接拒绝 → 符号链接用例失败；②通知去重关掉 → "同值不发"用例失败；
  ③淘汰改成保留 100 条旧记录 → 容量用例失败。

## 7. 交付记录（2026-10-17）

- **实现**：`host-services/src/gen_ui/`（`paths` / `schema` / `document` / `state` / `mod` + 四个测试文件，
  各自 ≤400 行）。`host-services` 因此新增依赖 `libc`（`O_NOFOLLOW`；与 host-rpc 里 `project_memory` 的
  既有做法一致，`libc` 已在工作区依赖表内）。跨进程锁复用 `escode-cli-host::file_lock`
  （其 `DEFAULT_TIMING` 与 TS `acquireFileLock(file, [25,50,100,200,400], 100, 8000)` 逐值相同）。
- **RPC 面**：`host-rpc/src/domains_gen_ui.rs`（4 个方法与 `onStateChanged` 事件）；缺 `dataBaseDir` 时
  方法表照常登记、调用回结构化缺口；`host-proto` `IMPLEMENTED` 登记 5 个条目。
- **测试**：`schema_tests`（4 例）、`paths_tests`（3 例）、`document_tests`（6 例，含真符号链接）、
  `state_tests`（5 例，含 watcher 通知与容量淘汰）；`tests/rpc_gen_ui.rs`（真 WebSocket 端到端）。
- **负向验证**（三项，均已恢复）：①去掉逐级符号链接拒绝 → 符号链接用例失败；②关掉通知去重 →
  端到端「同值不发」失败；③关掉容量淘汰 → 淘汰用例失败（101 vs 100）。
- **实现期发现的两处**：
  1. **事件帧可能先于触发它的那次写调用的应答到达**（`setState` 的 publish 与响应是两条独立出站路径）。
     首版测试在等应答时把事件帧丢掉了，导致后续断言空等——测试改为**缓冲事件帧**；
  2. `gen-ui` 的 `size > 5 MB` 检查在**读取之前**（TS 同序），所以静态超限文件拿到的是
     `Gen UI requires a regular file under 5 MB`，而 `Gen UI file exceeds 5 MB` 只属于
     「打开后文件又长大」的竞态路径。首版测试的预期写反了。

## 8. `prepareSandbox`（2026-10-21）

- **分工**：Host 负责壳 HTML、初始状态与 CSP 组装（与 TS `createGenUiService.prepareSandbox` 同序）；
  Electron Main 通过 `registerFromHost` 注册真实 webview 沙箱，Host 不自建 Electron 资源。
- **协议**：Host→Main 控制通道 `mainRequest { method: "gen-ui.prepareSandbox", params }`，
  Main 回 `mainResponse { ok, result }`；Main 未声明 `mainServices` 时方法回显式 `NotImplemented`。
- **参数整形**：`appIdentity = sha256(JSON.stringify(["gen-ui", JSON.stringify([identity||path, sessionId]), path]))`；
  初始状态先于运行时脚本注入；完整文档（`<html>/<head>/<body>`）在 Host 侧拒绝。
- **登记差异**：`runtimeId` 按 Host 进程生成一次（TS 按服务实例生成），对注册结果不可见。
