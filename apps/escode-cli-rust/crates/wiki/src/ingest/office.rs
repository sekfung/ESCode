//! DOC / XLS（OLE2 旧格式）经可选转换器转成 DOCX / XLSX 再解析（spec §7.4–§7.5）。
//! 转换器只来自**用户层**配置（项目层不可设置，见 settings.rs），以参数数组调用、隔离临时目录、超时即终止；
//! 未配置时明确失败 `converter_unavailable`，绝不把旧格式当作空文档。
use super::Failure;
use std::path::PathBuf;
use std::time::Duration;

pub async fn convert_legacy(
    bytes: &[u8],
    format: &str,
    converter: Option<&(String, u64)>,
) -> Result<Vec<u8>, Failure> {
    let Some((command, timeout_ms)) = converter else {
        return Err(Failure::new(
            "convert",
            format!(
                "converter_unavailable: {format} needs an office converter; set wiki.converters.office.enabled=true (LibreOffice `soffice`) in ~/.escode/cli/config.json"
            ),
        ));
    };
    let target = if format == "xls" { "xlsx" } else { "docx" };
    let dir = std::env::temp_dir().join(format!(
        "escode-wiki-convert-{}-{}",
        std::process::id(),
        nanos()
    ));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Failure::new("convert", e.to_string()))?;
    let result = run(&dir, bytes, format, target, command, *timeout_ms).await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    result
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

async fn run(
    dir: &PathBuf,
    bytes: &[u8],
    format: &str,
    target: &str,
    command: &str,
    timeout_ms: u64,
) -> Result<Vec<u8>, Failure> {
    let input = dir.join(format!("source.{format}"));
    tokio::fs::write(&input, bytes)
        .await
        .map_err(|e| Failure::new("convert", e.to_string()))?;
    let mut child = tokio::process::Command::new(command)
        .args([
            "--headless",
            "--norestore",
            "--convert-to",
            target,
            "--outdir",
        ])
        .arg(dir)
        .arg(&input)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            Failure::new(
                "convert",
                format!("cannot start converter '{command}': {e}"),
            )
        })?;
    let status = match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(status) => status.map_err(|e| Failure::new("convert", e.to_string()))?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(Failure::new(
                "convert",
                format!("converter timed out after {timeout_ms} ms"),
            ));
        }
    };
    if !status.success() {
        return Err(Failure::new(
            "convert",
            format!("converter exited with {status}"),
        ));
    }
    tokio::fs::read(dir.join(format!("source.{target}")))
        .await
        .map_err(|_| Failure::new("convert", "converter produced no output file"))
}
