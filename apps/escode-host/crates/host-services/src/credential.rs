//! `credential` 域（docs/specs/rust-u2-credential.md）：Host 侧凭据读写，复用与 Node 互通的 CLI `CredentialStore`。

use std::path::PathBuf;

use escode_cli_host::credential_cipher::CredentialCipher;
use escode_cli_host::credential_store::CredentialStore;

/// 解密失败的错误码与前缀（TS `CREDENTIAL_DECRYPT_ERROR_CODE` / `CREDENTIAL_DECRYPT_ERROR_PREFIX`）。
pub const DECRYPT_ERROR_CODE: &str = "ESCODE_CREDENTIAL_DECRYPT_FAILED";
const DECRYPT_ERROR_PREFIX: &str = "凭据解密失败：";

/// 服务错误：`code` 仅解密失败时存在。
#[derive(Debug)]
pub struct CredentialError {
    pub message: String,
    pub code: Option<&'static str>,
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CredentialError {}

/// CLI 密码器的英文错误 → TS 文案。
fn map_error(error: &anyhow::Error) -> CredentialError {
    let text = format!("{error:#}");
    const REASONS: [(&str, &str); 4] = [
        ("invalid ciphertext format", "密文格式非法"),
        ("invalid IV length", "IV 长度非法"),
        ("invalid auth tag length", "AuthTag 长度非法"),
        (
            "key mismatch or corrupted ciphertext",
            "密钥不匹配或密文已损坏",
        ),
    ];
    if let Some((_, reason)) = REASONS
        .iter()
        .find(|(english, _)| text.contains("Credential decrypt failed") && text.contains(english))
    {
        return CredentialError {
            message: format!("{DECRYPT_ERROR_PREFIX}{reason}"),
            code: Some(DECRYPT_ERROR_CODE),
        };
    }
    // base64 解码失败等同样是“密文格式非法”。
    if text.contains("Invalid symbol")
        || text.contains("Invalid padding")
        || text.contains("Invalid last symbol")
    {
        return CredentialError {
            message: format!("{DECRYPT_ERROR_PREFIX}密文格式非法"),
            code: Some(DECRYPT_ERROR_CODE),
        };
    }
    CredentialError {
        message: text,
        code: None,
    }
}

pub struct CredentialService {
    store: CredentialStore,
}

impl CredentialService {
    /// `<dataBaseDir>/.escode/v2/credentials.json`（TS `getAppConfigDir()`）。
    pub fn path(data_base_dir: &std::path::Path) -> PathBuf {
        data_base_dir
            .join(".escode")
            .join("v2")
            .join("credentials.json")
    }

    pub fn new(path: PathBuf, secret: &str) -> Self {
        Self {
            store: CredentialStore::new(path, CredentialCipher::new(secret)),
        }
    }

    pub async fn load(&self, key: &str) -> Result<Option<String>, CredentialError> {
        self.store
            .load(key)
            .await
            .map_err(|error| map_error(&error))
    }

    pub async fn save(&self, key: &str, value: &str) -> Result<(), CredentialError> {
        self.store
            .save_value(key, value)
            .await
            .map_err(|error| map_error(&error))
    }

    pub async fn delete(&self, key: &str) -> Result<(), CredentialError> {
        self.store
            .delete(key)
            .await
            .map_err(|error| map_error(&error))
    }
}

#[cfg(test)]
#[path = "credential_tests.rs"]
mod tests;
