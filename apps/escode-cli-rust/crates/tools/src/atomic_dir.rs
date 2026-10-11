//! 目录原子激活与崩溃恢复，对齐 TS `adapters/src/plugins/atomic-directory.ts`
//! （docs/specs/rust-plugin-marketplace-write.md W1b）。插件缓存由 Node 与 Rust 共享，所以事务标记
//! `.<name>.transaction.json`（v2）、备份 `.<name>.backup`、暂存 `.<name>.stage-*` 的布局与恢复规则必须与 TS
//! 一致：任一 runtime 崩溃留下的半成品，另一方都能按同一规则收拾。全部是同步 IO，调用方放到阻塞线程。

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn recovery_paths(target: &Path) -> (PathBuf, PathBuf) {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (
        parent.join(format!(".{name}.backup")),
        parent.join(format!(".{name}.transaction.json")),
    )
}

/// 进程级 owner id（TS `atomicProcessOwnerId`）：同 pid 的标记只有 owner id 相同才算本进程活跃事务。
fn owner_id() -> &'static str {
    static OWNER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OWNER.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

fn remove_all(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// TS `authorityContainsTransactionSync`：权威状态文件（installed_plugins.json）里任一处
/// `cacheTransactionId === id` 即视为已提交。
fn authority_contains(authority: &Path, transaction_id: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(authority) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let mut pending = vec![&value];
    while let Some(current) = pending.pop() {
        match current {
            Value::Object(map) => {
                if map.get("cacheTransactionId").and_then(Value::as_str) == Some(transaction_id) {
                    return true;
                }
                pending.extend(map.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    false
}

/// TS `recoverAtomicTargetSync`（无活跃写者时的恢复分支）：返回可读路径。活跃写者（其它存活进程）的事务
/// 不动，按已提交与否返回 target 或 backup。
pub(super) fn recover(target: &Path) -> PathBuf {
    let (backup, marker) = recovery_paths(target);
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let transaction: Option<Value> = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let v2 = transaction.as_ref().filter(|t| {
        t["version"] == 2
            && t["stageName"].is_string()
            && t["transactionId"].is_string()
            && t["ownerId"].is_string()
            && t["ownerPid"].as_u64().is_some_and(|pid| pid > 0)
            && t["hadTarget"].is_boolean()
            && matches!(t["mode"].as_str(), Some("coordinated" | "standalone"))
    });
    if let Some(t) = v2 {
        let pid = t["ownerPid"].as_u64().unwrap_or_default() as u32;
        let alive = if pid == std::process::id() {
            t["ownerId"].as_str() == Some(owner_id())
        } else {
            escode_cli_host::file_lock::process_alive(pid)
        };
        if alive {
            // TS resolveLiveAtomicReadPath：不能抢走仍在写的事务的回滚快照。
            let committed = t["mode"] == "coordinated"
                && t["authorityPath"].as_str().is_some_and(|authority| {
                    authority_contains(Path::new(authority), t["transactionId"].as_str().unwrap())
                });
            if committed {
                return target.to_owned();
            }
            if backup.exists() {
                return backup;
            }
            if t["hadTarget"] == true && target.exists() {
                return target.to_owned();
            }
            return backup;
        }
    }
    let coordinated_committed = v2.is_some_and(|t| {
        t["mode"] == "coordinated"
            && t["authorityPath"].as_str().is_some_and(|authority| {
                authority_contains(Path::new(authority), t["transactionId"].as_str().unwrap())
            })
    });
    match v2 {
        Some(t) if t["mode"] == "coordinated" => {
            if coordinated_committed {
                if target.exists() && backup.exists() {
                    remove_all(&backup);
                } else if !target.exists() && backup.exists() {
                    let _ = std::fs::rename(&backup, target);
                }
            } else if backup.exists() {
                remove_all(target);
                let _ = std::fs::rename(&backup, target);
            } else if t["hadTarget"] != true {
                remove_all(target);
            }
        }
        _ => {
            if !target.exists() && backup.exists() {
                let _ = std::fs::rename(&backup, target);
            } else if target.exists() && backup.exists() {
                remove_all(&backup);
            }
        }
    }
    if let Some(stage) = transaction
        .as_ref()
        .and_then(|t| t["stageName"].as_str())
        .filter(|stage| stage.starts_with(&format!(".{name}.stage-")))
    {
        remove_all(&parent.join(stage));
    }
    let _ = std::fs::remove_file(&marker);
    target.to_owned()
}

/// 一次已提交但尚未落定（finalize / rollback）的激活。
pub(super) struct Activation {
    target: PathBuf,
    backup: PathBuf,
    marker: PathBuf,
    target_moved: bool,
    pub transaction_id: String,
}

impl Activation {
    /// 权威状态（带同一 transactionId）落盘后：删备份与事务标记。
    pub(super) fn finalize(self) {
        if self.target_moved {
            remove_all(&self.backup);
        }
        let _ = std::fs::remove_file(&self.marker);
    }

    /// 依赖闭包后续插件或状态写入失败：撤销已激活目录，还原备份。
    pub(super) fn rollback(self) -> Result<()> {
        remove_all(&self.target);
        if self.target_moved {
            std::fs::rename(&self.backup, &self.target)
                .with_context(|| format!("Unable to restore {}", self.target.display()))?;
        }
        let _ = std::fs::remove_file(&self.marker);
        Ok(())
    }
}

/// TS `activateDirectoryAtomically`：把 `source` 复制到同目录暂存，`prepare` 补全后，排他写事务标记，
/// 旧目标改名为备份，暂存改名为目标。`authority` 是记录 `cacheTransactionId` 的权威状态文件。
pub(super) fn activate(
    source: Option<&Path>,
    target: &Path,
    authority: &Path,
    prepare: impl FnOnce(&Path) -> Result<()>,
) -> Result<Activation> {
    let parent = target
        .parent()
        .context("Target requires a parent directory")?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    std::fs::create_dir_all(parent)?;
    recover(target);
    let stage_container = parent.join(format!(".{name}.stage-{}", uuid::Uuid::new_v4().simple()));
    let staged = stage_container.join("content");
    let (backup, marker) = recovery_paths(target);
    let transaction_id = uuid::Uuid::new_v4().to_string();
    let result = (|| -> Result<Activation> {
        std::fs::create_dir_all(&stage_container)?;
        // URL / settings 市场只有规范化 manifest，没有可复制的源树：空暂存目录 + prepare。
        match source {
            Some(source) => copy_dir(source, &staged)?,
            None => std::fs::create_dir_all(&staged)?,
        }
        prepare(&staged)?;
        let had_target = target.exists();
        let record = json!({
            "authorityPath": std::path::absolute(authority).unwrap_or_else(|_| authority.to_owned()),
            "hadTarget": had_target,
            "mode": "coordinated",
            "ownerId": owner_id(),
            "ownerPid": std::process::id(),
            "stageName": stage_container.file_name().map(|n| n.to_string_lossy().into_owned()),
            "transactionId": transaction_id,
            "version": 2,
        });
        // 排他创建：另一个进程的存活事务不能被覆写（TS `flag: "wx"`）。
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        anyhow::anyhow!(
                            "Atomic directory activation is already active: {}",
                            target.display()
                        )
                    } else {
                        e.into()
                    }
                })?;
            file.write_all(format!("{record}\n").as_bytes())?;
        }
        let mut target_moved = false;
        if had_target {
            if let Err(error) = std::fs::rename(target, &backup) {
                let _ = std::fs::remove_file(&marker);
                return Err(error.into());
            }
            target_moved = true;
        }
        if let Err(error) = std::fs::rename(&staged, target) {
            if target_moved && !target.exists() {
                std::fs::rename(&backup, target)
                    .with_context(|| format!("Unable to restore {}", target.display()))?;
            }
            let _ = std::fs::remove_file(&marker);
            bail!(error);
        }
        Ok(Activation {
            target: target.to_owned(),
            backup: backup.clone(),
            marker: marker.clone(),
            target_moved,
            transaction_id: transaction_id.clone(),
        })
    })();
    remove_all(&stage_container);
    result
}

/// `cp -r`（TS `cp(source, staged, {recursive, force})`）：符号链接按链接本身复制。
pub(super) fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let to = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else if kind.is_symlink() {
            let link = std::fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &to)?;
            #[cfg(windows)]
            {
                if entry.path().is_dir() {
                    std::os::windows::fs::symlink_dir(&link, &to)?;
                } else {
                    std::os::windows::fs::symlink_file(&link, &to)?;
                }
            }
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn activate_replaces_target_and_rollback_restores_it() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        write(&source.join("a.txt"), "new");
        let target = dir.path().join("cache").join("p").join("1.0.0");
        write(&target.join("a.txt"), "old");
        let authority = dir.path().join("installed_plugins.json");
        let activation = activate(Some(&source), &target, &authority, |staged| {
            std::fs::write(staged.join("b.txt"), "prepared")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("a.txt")).unwrap(),
            "new"
        );
        assert!(target.join("b.txt").exists());
        let (backup, marker) = recovery_paths(&target);
        assert!(backup.exists() && marker.exists());
        activation.rollback().unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("a.txt")).unwrap(),
            "old"
        );
        assert!(!backup.exists() && !marker.exists());
        // 暂存目录已清理。
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, ["1.0.0"]);
    }

    #[test]
    fn finalize_drops_backup_and_recovery_of_dead_writer_restores_backup() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        write(&source.join("a.txt"), "new");
        let target = dir.path().join("t");
        write(&target.join("a.txt"), "old");
        let authority = dir.path().join("installed_plugins.json");
        activate(Some(&source), &target, &authority, |_| Ok(()))
            .unwrap()
            .finalize();
        let (backup, marker) = recovery_paths(&target);
        assert!(!backup.exists() && !marker.exists());
        // 模拟崩溃的写者（未提交）：新内容已在 target，旧内容在 backup，标记属于不存在的进程。
        std::fs::rename(&target, &backup).unwrap();
        write(&target.join("a.txt"), "half");
        let dead = json!({"version":2,"stageName":".t.stage-x","transactionId":"tx","ownerId":"o",
            "ownerPid":999_999_999u64,"hadTarget":true,"mode":"coordinated",
            "authorityPath": authority.to_string_lossy()});
        std::fs::write(&marker, dead.to_string()).unwrap();
        recover(&target);
        assert_eq!(
            std::fs::read_to_string(target.join("a.txt")).unwrap(),
            "new"
        );
        assert!(!marker.exists() && !backup.exists());
        // 已提交（权威状态带同一 transactionId）：保留 target，丢弃 backup。
        write(&backup.join("a.txt"), "older");
        std::fs::write(&marker, dead.to_string()).unwrap();
        std::fs::write(&authority, r#"{"plugins":[{"cacheTransactionId":"tx"}]}"#).unwrap();
        recover(&target);
        assert_eq!(
            std::fs::read_to_string(target.join("a.txt")).unwrap(),
            "new"
        );
        assert!(!backup.exists());
    }
}
