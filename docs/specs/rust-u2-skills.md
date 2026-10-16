# Rust Host `skills` 域（U2）

> 状态：交付 · 2026-10-16。TS 真相源：`packages/services/src/skills/{skillsService.ts,skillDiscoveryWalk.ts}`、`plugins/installedPluginRoots.ts`、
> `@escode/shared` 的 `skill-scan-policy.ts` / `plugin-marketplaces.ts`。

## 1. 范围（6 个成员）

`list`、`setEnabled`、`buildPromptContext`、`copyToCommon`、`removeFromCommon`、`deleteSkill`。

## 2. 事实源与所有者

- 技能文件：工作区各层 `.escode/skills` + `.agents/skills`（从 workspace 沿父目录走到含 `.git` 的目录，找不到则只用 workspace 自身）、用户级
  `~/.escode/skills` + `~/.agents/skills`（**仅桌面运行时**，由 `ESCODE_PROCESS_LABEL` 判定，否则 `capability = {userScopeAvailable:false, userScopeReason:"desktop_only"}`）、
  以及启用的插件的技能根。`~` = Host 家目录。
- 启停开关：`~/.escode/cli/config.json` 的 `skills.<SKILL.md 路径>.enable`（只写 `false`，开启 = 删除 override；其余键与键序原样保留——用 `escode_cli_domain::json_order::Json`）。
- 审计：`buildPromptContext` 命中技能时追加 `~/.escode/v2/skills-audit.log`。
- 插件根（`plugins.enabled` 缺省 true）：`plugins.dirs`（默认启用）、官方缓存 `<storage>/cli/plugins/cache/zcode-plugins-official/<插件>/<版本>`（仅默认启用清单内的才开）、
  `installed_plugins.json`（要求绝对 `installPath`）；清单取 `.escode-plugin` / `.zcode-plugin` / `.claude-plugin` / `.codex-plugin` 下的 `plugin.json`，名字须匹配
  `^[a-z0-9][a-z0-9._-]{0,127}$`；`skills` 字段的相对路径不得越出插件根，缺省用 `skills/`；`suppressedBuiltins` 与 `enabledPlugins` 覆盖默认。

## 3. 解析与发现

- **frontmatter**：统一换行后取 `^---\n…\n---`；Markdown 标题行（`#{1,6}\s+\S`）之后的内容视为“泄漏到 frontmatter 里的正文”移回 body；
  严格解析用 saphyr（YAML 1.2 core）。历史写法回退到宽松逐行读取、`parseOk=false`：未加引号的 `name:`/`description:` 值含 `: `、YAML 报错、非映射、重复顶层键、多文档。
- **遍历**：DFS，目录项按字节序（libuv `scandir` 的 `alphasort`），跳过 `node_modules/dist/build/out/target/vendor/coverage/.cache/.next/.turbo/.venv/__pycache__` 与点目录（`.system` 除外），
  最大深度 8；符号链接目录按 realpath 去重。扫描根先去“被更深层已存在根覆盖的祖先”，再按 realpath 去重。
- **条目**：id = `glm:<scope>:<name>:<sha256(规范路径)[0..12]>`；name 缺省取目录名；description > 1024 UTF-16 码元 → 诊断并跳过；`_meta.json` 的 `slug/version/ownerId/publishedAt`。
  用户 `~/.agents/skills` 里被 `~/.escode/skills` 同目录名或同 name 覆盖的技能不列出。排序：name、scope、path（默认 locale 的 `localeCompare`）。
- **变更**：`copyToCommon`（不覆盖；符号链接按链接复制）、`removeFromCommon`（必须在通用根内）、`deleteSkill`（拒绝 plugin 技能；父目录 realpath 后须落在受控根内；叶子是软链只删链接）。

## 4. 验收

- **TS 黄金样本**：`scripts/generate-escode-host-skills-fixtures.mjs`（`--check` 已并入 `check:escode-host`）从真实 TS 导出 42 份 `SKILL.md` 的 `readFrontmatter` 结果
  （CRLF/CR、未加引号的中文冒号、块标量、泄漏标题、重复键、多文档、BOM、非映射…）与扫描策略/默认启用清单；Rust 单测逐项相等。
- **全链路差分**：`rustHostSkills.test.ts` 在同一棵真实目录树上让 TS 服务与 Rust Host 的 `list` 精确相等（含 id 哈希），再在两棵同构树上依次执行
  启停/提示词上下文/复制/移除/删除（含符号链接、插件技能、未知技能），逐步比较结果、目录树与配置文件字节。
- 负向验证：去掉默认启用官方插件的并集、反转启停写入，均失败。

## 5. 登记差异（不修）

1. YAML 差异：saphyr 与 TS `yaml` 的错误边界不完全相同（语料已覆盖常见历史写法），数值类标量的 `String(value)` 只复刻整数/普通小数/布尔/null/序列/映射。
2. 扫描错误的 `message` 是 Rust 的 `io::Error` 文案，TS 是 Node 的 `ENOENT: …`（诊断 code/severity/path 一致）。
3. Windows 路径规则（盘符、反斜杠分隔的 `normalizeScanRootPath`）只覆盖 POSIX 路径。
