//! 远端同步的目录写权限探测（TS `remoteSyncWriteAccess.ts`）。
//!
//! skill/mcp 同步共用：`mkdir -p` 后独占创建标记文件、写入 `ok`、删除。
//! 失败不抛错，返回 `{ok:false,path,error}` 供 UI 展示。

use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

pub fn check_directory_write_access(directory: &Path) -> Value {
    let marker = directory.join(format!(
        ".escode-sync-preflight-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(directory)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?;
        file.write_all(b"ok")?;
        std::fs::remove_file(&marker)?;
        Ok(())
    })();
    match result {
        Ok(()) => json!({"ok": true, "path": directory.to_string_lossy()}),
        Err(error) => {
            let _ = std::fs::remove_file(&marker);
            json!({
                "ok": false,
                "path": directory.to_string_lossy(),
                "error": error.to_string(),
            })
        }
    }
}

#[cfg(test)]
#[path = "remote_sync_tests.rs"]
mod tests;
