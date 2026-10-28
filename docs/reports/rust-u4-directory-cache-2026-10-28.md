# Rust 阶段 4：skill/plugin 目录扫描共享结果（2026-10-28）

父计划：`docs/specs/rust-unified-host.md` §阶段 4「跨实例共享缓存」。
契约：`docs/specs/rust-u4-directory-cache.md`。

## 1. 结论

技能发现的**目录遍历仍然每次执行**（每个 workspace 的根集合不同），但同一目录的
`SKILL.md` 内容读取、frontmatter 解析与同级 `_meta.json` 读取现在按「目录 + 指纹」
在 Host 进程内共享。用户级根、同 repo 的祖先目录根和插件贡献根在多个 workspace 之间
命中同一份解析结果。

- 缓存键：归一化的扫描根目录（与 `dedupe_scan_roots` 一致）。
- 指纹：本次 walk 得到的 `SKILL.md` 路径集合，逐个计入 `(path, size, mtime_ms)`；
  同级 `_meta.json` 存在时同样计入。
- 失效：新增/删除技能、大小或 mtime 变化、`_meta.json` 变化都会失配并重新解析；
  不依赖 watcher、超时或内容哈希。
- 隔离：`scope`/`pluginName`/`pluginId`/`id` 按当前 root 重新生成；
  `covered_by_escode`、canonical 去重与存在性检查保持在缓存之外。

## 2. 变更

- 新增 `apps/escode-host/crates/host-services/src/skills/cache.rs`：
  `scan_root_cached(root, compare, diagnostics)`，进程级 `OnceLock<Mutex<HashMap>>`，
  条目上限 256（超出整体清空）。
- `skills/discover.rs`：逐根扫描改为读取缓存条目后物化；删除重复的解析/`_meta.json` 读取。
- 新增 `skills/cache_tests.rs`：7 个用例覆盖命中、大小/mtime/元数据失效、增删技能、
  目录隔离、scope 物化与 `~/.agents` 遮蔽过滤。

## 3. 验证

```
cargo fmt --manifest-path apps/escode-host/Cargo.toml --all
cargo test  --manifest-path apps/escode-host/Cargo.toml -p escode-host-services
cargo clippy --manifest-path apps/escode-host/Cargo.toml -p escode-host-services --all-targets -- -D warnings
```

- `escode-host-services`：539 通过 / 0 失败；cache 子集 7 通过。
- `cargo test --workspace`：550 通过 / 0 失败。
- clippy `-D warnings`（workspace）：通过；`cargo fmt --all --check`：通过。
- 未跑 `pnpm test:escode-host` 里的 TS 差分与 `check:escode-host` 生成物校验；Rust 侧编译、
  workspace 测试与 clippy 已覆盖本切片。技能 golden fixture 若在 CI 上出现顺序差异，
  按 `cache` 命中路径与解析路径的同序输出排查（两者都走 `compare` 排序）。

## 4. 已知差异

- 指纹使用毫秒 mtime，与工具层 `FileStamp` 同级；同一毫秒内等大小原地改写可能命中旧条目。
- 目录 walk 仍每次执行；本切片只消除内容读取与解析的重复成本。
- 缓存不跨进程：`app-server --stdio` 链路不受影响。
- 超过 256 个目录时整体清空，下一次全量解析；只影响延迟。
