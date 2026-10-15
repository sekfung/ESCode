//! Personal 文件的带锁读-改-写（TS `NodePersonalProviderConfigRepository.update`）。
//!
//! 与 TS 同协议：`<file>.lock` 目录锁（跨进程）+ 进程内 FIFO；锁内重读最新文件、应用变换、
//! 规范化校验、临时文件 + rename 私有原子写（0600）。变换失败或选择“不写”时不动文件。

use std::path::Path;

use escode_cli_host::file_lock;
use serde_json::Value;

use super::personal::Doc;

/// 变更失败的统一错误：文案原样回给 Renderer（`RpcError("Error", message)`）。
#[derive(Debug)]
pub struct MutationError(pub String);

impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MutationError {}

impl From<String> for MutationError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for MutationError {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

/// 变换的结论：`Commit` 落盘；`Skip` 保持文件不变（TS `renamePersonalModel` 同名直接返回）。
pub enum Write {
    Commit,
    Skip,
}

/// 读取当前文件；不存在 = 空配置，存在但无效 = 报错（写入路径不静默覆盖用户文件）。
async fn read_locked(path: &Path) -> Result<Doc, MutationError> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text).map_err(|error| {
                MutationError(format!("Personal Provider Config 不是合法 JSON: {error}"))
            })?;
            Doc::decode(&value).map_err(|error| MutationError(format!("{error:#}")))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Doc::empty()),
        Err(error) => Err(MutationError(format!(
            "读取 Personal Provider Config 失败: {error}"
        ))),
    }
}

/// 在文件锁内执行 `transform`；返回其结果值。
pub async fn update<T>(
    path: &Path,
    transform: impl FnOnce(&mut Doc) -> Result<(Write, T), MutationError>,
) -> Result<T, MutationError> {
    let guard = file_lock::acquire(path)
        .await
        .map_err(|error| MutationError(format!("{error:#}")))?;
    let outcome = run(path, transform).await;
    guard.release().await;
    outcome
}

async fn run<T>(
    path: &Path,
    transform: impl FnOnce(&mut Doc) -> Result<(Write, T), MutationError>,
) -> Result<T, MutationError> {
    let mut doc = read_locked(path).await?;
    let (write, value) = transform(&mut doc)?;
    if matches!(write, Write::Commit) {
        let (_, encoded) = doc
            .canonical()
            .map_err(|error| MutationError(format!("{error:#}")))?;
        let text = serde_json::to_string_pretty(&encoded)
            .map_err(|error| MutationError(error.to_string()))?;
        file_lock::write_private(path, &text)
            .await
            .map_err(|error| {
                MutationError(format!("写入 Personal Provider Config 失败: {error:#}"))
            })?;
    }
    Ok(value)
}
