//! 与 Node 共用的凭据文件（docs/specs/rust-mcp-oauth.md，对齐 TS `adapters/src/auth/shared-credentials.ts`）：
//! `<ZCODE_DATA_BASE_DIR 或 homedir>/.zcode/v2/credentials.json`，JSON 对象，值为加密字符串。
//! 修改始终在跨进程文件锁内 read-modify-write，并原子写回；损坏文件先按内容备份再失败，绝不当成空对象覆盖。
use super::{credential_cipher::CredentialCipher, file_lock};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};

pub struct CredentialStore {
    path: PathBuf,
    cipher: CredentialCipher,
}

/// TS `resolveSharedZCodeCredentialsPath`。
pub fn default_path() -> PathBuf {
    let base = std::env::var("ZCODE_DATA_BASE_DIR")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(super::credential_cipher::node_homedir);
    let base = match base.strip_prefix("~/") {
        Some(rest) => PathBuf::from(super::credential_cipher::node_homedir()).join(rest),
        None if base == "~" => PathBuf::from(super::credential_cipher::node_homedir()),
        None => std::path::absolute(&base).unwrap_or_else(|_| PathBuf::from(&base)),
    };
    base.join(".zcode").join("v2").join("credentials.json")
}

type Raw = Map<String, Value>;

impl CredentialStore {
    pub fn new(path: PathBuf, cipher: CredentialCipher) -> Self {
        Self { path, cipher }
    }
    pub fn from_environment() -> Self {
        Self::new(default_path(), CredentialCipher::from_environment())
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    async fn read_raw(&self) -> Result<Raw> {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Raw::new()),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "Unable to read shared ZCode credentials: {}",
                        self.path.display()
                    )
                });
            }
        };
        let parsed: Option<Raw> = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .filter(|map| map.values().all(Value::is_string));
        match parsed {
            Some(map) => Ok(map),
            None => {
                let backup = self.backup_corrupt(&bytes).await.ok();
                bail!(
                    "Shared ZCode credentials are corrupt: {}.{}",
                    self.path.display(),
                    backup
                        .map(|b| format!(" Backup: {}", b.display()))
                        .unwrap_or_default()
                )
            }
        }
    }
    /// TS `backupCorruptFile`：按内容 hash 命名、排他创建，重复失败收敛到同一份证据。
    async fn backup_corrupt(&self, bytes: &[u8]) -> Result<PathBuf> {
        let id: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let backup = PathBuf::from(format!("{}.corrupt-{}.bak", self.path.display(), &id[..24]));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&backup).await {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(bytes).await?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        Ok(backup)
    }
    fn decrypt(&self, raw: &Raw, key: &str) -> Result<Option<String>> {
        raw.get(key)
            .and_then(Value::as_str)
            .map(|v| self.cipher.decrypt(v))
            .transpose()
    }
    pub async fn load(&self, key: &str) -> Result<Option<String>> {
        self.decrypt(&self.read_raw().await?, key.trim())
    }
    pub async fn load_many(&self, keys: &[&str]) -> Result<BTreeMap<String, Option<String>>> {
        let raw = self.read_raw().await?;
        keys.iter()
            .map(|key| Ok((key.trim().to_owned(), self.decrypt(&raw, key.trim())?)))
            .collect()
    }
    /// 锁内 read-modify-write；`mutate` 返回 false 表示无需写回。
    async fn mutate(
        &self,
        mutate: impl FnOnce(&mut Raw, &CredentialCipher) -> Result<bool>,
    ) -> Result<bool> {
        let guard = file_lock::acquire(&self.path).await?;
        let result = async {
            let mut raw = self.read_raw().await?;
            if !mutate(&mut raw, &self.cipher)? {
                return Ok(false);
            }
            // TS `JSON.stringify(value, null, 2) + "\n"`（serde_json 保序输出同样两空格缩进）。
            let text = format!("{}\n", serde_json::to_string_pretty(&Value::Object(raw))?);
            file_lock::write_private(&self.path, &text).await?;
            Ok(true)
        }
        .await;
        guard.release().await;
        result
    }
    pub async fn save_many(&self, entries: &[(String, String)]) -> Result<()> {
        for (key, value) in entries {
            ensure!(!key.trim().is_empty(), "Credential key must not be empty");
            ensure!(!value.is_empty(), "Credential value must not be empty");
        }
        let sealed = entries
            .iter()
            .map(|(k, v)| Ok((k.trim().to_owned(), self.cipher.encrypt(v)?)))
            .collect::<Result<Vec<_>>>()?;
        self.mutate(move |raw, _| {
            for (key, value) in sealed {
                raw.insert(key, Value::String(value));
            }
            Ok(true)
        })
        .await?;
        Ok(())
    }
    /// TS `deleteManyIfValue`：guard 当前值等于期望值才整体删除，否则一个都不删。
    pub async fn delete_many_if_value(
        &self,
        guard: &str,
        expected: &str,
        keys: &[String],
    ) -> Result<bool> {
        let guard = guard.trim().to_owned();
        let expected = expected.to_owned();
        let keys: Vec<String> = keys.iter().map(|k| k.trim().to_owned()).collect();
        self.mutate(move |raw, cipher| {
            let current = raw
                .get(&guard)
                .and_then(Value::as_str)
                .map(|v| cipher.decrypt(v))
                .transpose()?;
            if current.as_deref() != Some(expected.as_str()) {
                return Ok(false);
            }
            for key in keys {
                raw.remove(&key);
            }
            Ok(true)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn locked_writes_round_trip_and_guarded_delete_is_atomic() {
        let dir = std::env::temp_dir().join(format!("zcode-cred-{}", crate::id()));
        let store = CredentialStore::new(dir.join("credentials.json"), CredentialCipher::new("s"));
        store
            .save_many(&[("a".into(), "1".into()), ("b".into(), "2".into())])
            .await
            .unwrap();
        assert_eq!(store.load("a").await.unwrap().as_deref(), Some("1"));
        assert!(
            !store
                .delete_many_if_value("a", "x", &["a".into(), "b".into()])
                .await
                .unwrap()
        );
        assert!(
            store
                .delete_many_if_value("a", "1", &["a".into(), "b".into()])
                .await
                .unwrap()
        );
        assert_eq!(store.load("b").await.unwrap(), None);
        assert!(!dir.join("credentials.json.lock").exists());
        tokio::fs::write(dir.join("credentials.json"), "not json")
            .await
            .unwrap();
        assert!(
            store
                .load("a")
                .await
                .unwrap_err()
                .to_string()
                .contains("corrupt")
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
