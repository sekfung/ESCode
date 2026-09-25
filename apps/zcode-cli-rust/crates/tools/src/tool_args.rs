//! 工具参数解析与校验（与 TS 工具输入 schema 的宽松程度一致）。
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .with_context(|| format!("{key} must be a string"))
}
pub(super) fn uint(args: &Value, key: &str, default: u64) -> Result<u64> {
    match args.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= 9_007_199_254_740_991)
            .with_context(|| format!("{key} must be a nonnegative integer")),
    }
}
pub(super) fn boolean(args: &Value, key: &str, default: bool) -> Result<bool> {
    match args.get(key) {
        None => Ok(default),
        Some(Value::Bool(v)) => Ok(*v),
        Some(v) => match v.as_str().map(|s| s.trim().to_lowercase()).as_deref() {
            Some("true" | "1" | "yes" | "y" | "on") => Ok(true),
            Some("false" | "0" | "no" | "n" | "off") => Ok(false),
            _ if v == 1 => Ok(true),
            _ if v == 0 => Ok(false),
            _ => bail!("{key} must be boolean"),
        },
    }
}
pub(super) fn keys(args: &Value, allowed: &[&str]) -> Result<()> {
    let object = args.as_object().context("Tool arguments must be object")?;
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        bail!("Unsupported argument: {key}");
    }
    Ok(())
}
pub(super) fn resolve(cwd: &Path, input: &str) -> Result<PathBuf> {
    if input.trim().is_empty() || input.contains('\0') {
        bail!("Tool path must not be empty or contain NUL");
    }
    let p = Path::new(input);
    Ok(if p.is_absolute() {
        p.to_owned()
    } else {
        cwd.join(p)
    })
}
