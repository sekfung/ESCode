# Rust 阶段 4：skill/plugin 目录扫描共享（rust-unified-host.md 超越项 2 补充）

父计划：`docs/specs/rust-unified-host.md` §阶段 4「跨实例共享缓存」。
前置：`docs/specs/rust-u4-shared-cache.md`（provider/模型注册表共享）、
`docs/specs/rust-u2-skills.md`（技能发现语义）。

## 1. 问题与目标

嵌入 Host 内每个 workspace 一个 runtime 实例；每实例的 `SkillsService` 都会重新遍历并解析：

- 用户级根：`~/.escode/skills`、`~/.agents/skills`（所有 workspace 完全相同）；
- 项目级根：当前 workspace 及其祖先目录的 `.escode/skills`、`.agents/skills`（同一 repo 的多个
  workspace 大量重叠）；
- 插件贡献根（`plugin_roots`，按配置解析，同一插件目录被多个 workspace 共用）。

`walk` 的目录遍历无法跨实例复用（每个 workspace 的根集合不同），但**同一目录的文件读取、
frontmatter 解析与 `_meta.json` 读取**可以。目标：以「目录 + 目录指纹」为键做进程内共享，
命中时跳过内容读取与解析，同时保证 A 项目的技能不会泄漏给 B 项目。

本切片只缓存目录扫描结果；技能启用状态、`covered_by_escode` 去重、scope/plugin 字段仍由
每次发现调用按当前 workspace 计算。

## 2. 范围与非目标

范围内：

- `crates/host-services/src/skills/cache.rs`：进程级目录缓存（`static` + `Mutex`）。
- `skills/discover.rs`：扫描改为 `scan_root_cached`，缓存未命中时读取/解析并写入。
- 单元测试：同目录命中、内容/大小/mtime 失效、不同目录隔离、scope/plugin 重新物化。

非目标：

- **跨进程共享**：`app-server --stdio` 是独立进程，缓存不跨进程。
- **watcher 驱动失效**：本切片按每次发现调用重新计算指纹；Host 既有 watcher 不参与。
- **内容哈希**：指纹只用 `(size, mtime_ms)`，与工具层 `FileStamp` 一致；同一毫秒内
  保持大小不变的原地改写可能命中旧条目（见 §6）。
- **负缓存**：目录不存在、读取失败不缓存，保持每次都探测。
- **插件清单/配置解析**：只缓存技能目录扫描结果，不缓存插件配置读取。

## 3. 所有权与数据流

```
runtime A ─┐
runtime B ─┼─ discover() ──▶ scan_root_cached(root)
runtime C ─┘                     │
                                 ├─ walk(root) ──▶ 本次 SKILL.md 路径集合
                                 ├─ fingerprint(paths) = hash(排序后的 path,size,mtime_ms
                                 │                              + 同级 _meta.json)
                                 ├─ 命中 → 复用 CachedSkill[]（不读文件）
                                 └─ 未命中 → 读取 + frontmatter 解析 + _meta.json → 写缓存
                                        │
                                        ▼
                                 discover() 按当前 root 物化 scope/plugin 字段，
                                 再执行 covered_by_escode 与路径去重
```

- 缓存键：`canonical(root.path)`（与 `dedupe_scan_roots` 相同的路径归一化）。
- 缓存值：`{ fingerprint, Vec<CachedSkill>, Vec<scan diagnostics> }`。
- 所有权：缓存只存解析后的纯数据；`SkillsService`/runtime 生命周期不进入缓存，drop 不影响命中。
- 上限：条目数超过 256 时整体清空（避免长驻 Host 按临时目录无限增长）；清空只影响性能。

## 4. 契约

- **逐目录隔离**：缓存条目只包含该目录 `walk` 得到的 `SKILL.md`，不合并父/子目录结果。
- **指纹定义**：对本次 `walk` 结果排序后，逐个文件计入相对路径、`len`、`mtime_ms`；
  若同级存在 `_meta.json`，同样计入。任一项变化即未命中。
- **失效即时性**：新增/删除 `SKILL.md`、修改大小或 mtime、增删改 `_meta.json` 后，
  下一次 `discover` 必须返回新结果，不依赖 watcher 或超时。
- **字段物化**：`scope`、`pluginName`、`pluginId`、`id` 每次按传入的 root 重新生成；
  同目录以不同 root 身份扫描时，除这些字段外内容一致。
- **去重与遮蔽**：`covered_by_escode`、canonical 去重、`exists` 检查保持在缓存之外，
  按当前 workspace 的 `user_keys` 执行。
- **诊断**：`walk`/读取失败诊断随缓存条目保存；命中时按原顺序返回，失配时重新生成。
- **并发**：指纹计算在锁外；未命中时允许并发重复解析，最终写入等价结果。

## 5. 验收

| 用例                                   | 断言                                                                        |
| -------------------------------------- | --------------------------------------------------------------------------- |
| 同目录连续两次发现                     | 第二次命中缓存（命中计数 +1），技能结果逐字段相同                           |
| 修改 `SKILL.md` 内容（大小变）         | 下一次发现返回新 `description`/`body`                                       |
| 仅 touch mtime                         | 下一次发现重新读取（mtime 变化使指纹失配）                                  |
| 新增/删除 `SKILL.md`                   | 结果集合随之增删                                                            |
| 不同目录同名技能                       | 两条结果的 `path` 与 `id` 不同，互不覆盖                                    |
| 同目录以 workspace/user 两种 root 扫描 | `scope` 正确切换，内容一致                                                  |
| `covered_by_escode`                    | `~/.agents/skills` 中被 `~/.escode/skills` 覆盖的技能仍被过滤               |
| 缓存上限                               | 超过 256 个目录后仍返回正确结果（允许清空后重建）                           |
| 现有套件                               | `cargo test --manifest-path apps/escode-host/Cargo.toml --workspace` 不回退 |

## 6. 已知差异与风险

| 项           | 说明                                                                                   |
| ------------ | -------------------------------------------------------------------------------------- |
| 粗粒度 mtime | 某些文件系统 mtime 粒度为秒；同一秒内等大小原地改写可能命中旧内容。与 `FileStamp` 同级 |
| 无 watcher   | 指纹在每次发现时重算；空闲期间不主动刷新，但每次调用都是最新状态                       |
| 无跨进程     | child/stdio 链路不共享；与 provider 共享缓存边界一致                                   |
| 缓存清空抖动 | 超过上限整体清空，下一次全量解析；只影响延迟，不影响正确性                             |
