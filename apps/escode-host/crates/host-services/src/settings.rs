//! settings 服务域（rust-u2-host-skeleton.md §5 优先序 3）：Host 是设置的**唯一写者**，
//! 且与 TS Host 共用同一文件同一格式（`<home>/.escode/v2/setting.json`），
//! 两侧交替写读不破坏状态（阶段 2 验收「双 Host 交替写读演练」）。
//!
//! 语义对齐 TS `packages/services/src/setting/settingService.ts`：
//! - 读取：解析失败先短重试（3 × 300ms，读者可能撞上另一次写的半截 JSON），
//!   连续失败才把坏文件隔离为 `setting.json.corrupt-<时间戳>` 并回退默认值；文件缺失回退默认值；
//! - 写入：**顶层合并**（保留文件里其它键，含 TS 的 legacy 账户连接字段），
//!   2 空格缩进 JSON，`host-core::atomic_write`（临时文件 + fsync + rename 重试 + 父目录 fsync）。
//!
//! 本域不实现 TS 的 zod 校验与迁移（那是 TS Host 的职责）：Rust 侧只写调用方给出的补丁键，
//! 不重写整棵配置树，因此不会丢弃对方的字段——这是两端互操作的关键性质。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

/// 读取前的解析重试（与 TS 同参：读者可能读到半截 JSON）。
const PARSE_RETRY_COUNT: usize = 3;
const PARSE_RETRY_DELAY: Duration = Duration::from_millis(300);

/// 设置文件路径：`<home>/.escode/v2/setting.json`（SETTINGS_DIR + FILE，与 TS 同名同层）。
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(".escode").join("v2").join("setting.json")
}

/// TS `appSettingsSchema.parse({})` 的默认对象（zod 默认值；可选字段不出现在结果里）。
///
/// 修复：Rust 过去缺文件/坏文件返回 `{}`，TS 返回完整默认对象；切唯一 Host 后同一份设置会
/// 出现两种视图（例如 `locale`/`localePreference`/`terminalInheritSystemProfile` 缺失）。
/// `rustHostRpc.test.ts` 用真实客户端把本函数的结果与 TS schema 的 `parse({})` 做全等断言。
pub fn default_settings() -> Value {
    json!({
        "recentProjects": [],
        "locale": "zh-CN",
        "localePreference": "system",
        "terminalInheritSystemProfile": true,
        "embeddedBrowserAllowInsecureCertificates": false,
        "embeddedBrowserViewportPreference": {
            "mode": "normal",
            "viewport": {"width": 393, "height": 852},
            "zoom": "fit",
        },
        "computerUseComposerEntryHidden": true,
        "taskAutoArchiveEnabled": false,
        "taskAutoArchiveOlderThanDays": 7,
        "closeToTrayOnWindows": true,
        "closeToTrayOnWindowsMigrationInitialized": true,
        "closeToTrayLinuxMigrationInitialized": false,
        "keepAwakeWhileRunning": false,
        "desktopChromiumHardwareAccelerationEnabled": true,
        "messageStreamShowReasoning": true,
        "messageStreamShowReasoningMigrationInitialized": true,
        "messageStreamShowTodos": false,
        "toolGroupingExploreEnabled": true,
        "toolGroupingTerminalEnabled": true,
        "toolGroupingChangesEnabled": false,
        "escodeInteractionBehavior": "queue",
        "askUserQuestionAutoResolutionEnabled": true,
        "modelIoFullRetentionEnabled": false,
        "enabledBuiltinAgentCliProviders": ["glm"],
        "providerFamilyConnectionSelections": {},
        "providerFamilyDomainMigrated": false,
        "nativeSearchEnhancementsEnabled": true,
        "memoryEnabled": false,
        "lastWorkspaceSession": [],
        "lastActiveTabIndex": 0,
        "receivePreviewUpdates": false,
        "autoDownloadAndInstallUpdates": false,
        "skippedElectronUpdateVersions": {},
    })
}

/// 文件值覆盖默认值（TS `appSettingsSchema` 对缺省字段补默认；文件里的显式值优先）。
fn with_defaults(settings: Value) -> Value {
    let Value::Object(mut merged) = default_settings() else {
        unreachable!("default settings is an object")
    };
    if let Value::Object(object) = settings {
        for (key, value) in object {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}

/// 读取设置（对象形式）。坏文件按 TS 语义隔离后回退默认值；缺失返回默认值；
/// 合法对象按 TS schema 的缺省字段语义补齐默认值。
pub async fn read(home: &Path) -> Result<Value> {
    let path = settings_path(home);
    let mut raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_settings());
        }
        Err(_) => return Ok(default_settings()),
    };
    let mut parsed = serde_json::from_str::<Value>(&raw);
    let mut attempt = 0;
    while parsed.is_err() && attempt < PARSE_RETRY_COUNT {
        attempt += 1;
        tokio::time::sleep(PARSE_RETRY_DELAY).await;
        raw = match tokio::fs::read_to_string(&path).await {
            Ok(raw) => raw,
            Err(_) => break,
        };
        parsed = serde_json::from_str::<Value>(&raw);
    }
    match parsed {
        Ok(Value::Object(object)) => Ok(with_defaults(Value::Object(object))),
        Ok(_) => {
            // 合法 JSON 但不是对象：按 TS 的 schema 校验失败处理（不隔离，回退默认值）。
            Ok(default_settings())
        }
        Err(_) => {
            quarantine(&path).await;
            Ok(default_settings())
        }
    }
}

/// 应用补丁：顶层键合并（补丁里的键覆盖，文件里其余键原样保留），随后原子写回。
/// 返回写入后的完整设置对象。
pub async fn apply_patch(home: &Path, patch: &Value) -> Result<Value> {
    let patch = patch
        .as_object()
        .context("Settings patch must be a JSON object")?;
    let mut merged = match read(home).await? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    for (key, value) in patch {
        merged.insert(key.clone(), value.clone());
    }
    let merged = Value::Object(merged);
    let path = settings_path(home);
    let serialized = format!(
        "{}\n",
        serde_json::to_string_pretty(&merged).context("Settings serialization failed")?
    );
    escode_host_core::atomic_write::write(&path, serialized.as_bytes())
        .with_context(|| format!("Settings write failed: {}", path.display()))?;
    Ok(merged)
}

/// 设置单个顶层键（`None` = 移除，对应 TS `update({key: undefined})` 经 JSON 序列化丢键）。
pub async fn set_key(home: &Path, key: &str, value: Option<Value>) -> Result<Value> {
    let mut merged = match read(home).await? {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    match value {
        Some(value) => merged.insert(key.to_owned(), value),
        None => merged.remove(key),
    };
    let merged = Value::Object(merged);
    let path = settings_path(home);
    let serialized = format!(
        "{}\n",
        serde_json::to_string_pretty(&merged).context("Settings serialization failed")?
    );
    escode_host_core::atomic_write::write(&path, serialized.as_bytes())
        .with_context(|| format!("Settings write failed: {}", path.display()))?;
    Ok(merged)
}

/// 隔离坏文件（TS `quarantineCorruptSettingsFile`：`setting.json.corrupt-<ISO 时间戳>`）。
async fn quarantine(path: &Path) {
    let timestamp = chrono_timestamp();
    let stamp = timestamp.replace([':', '.'], "-");
    let target = PathBuf::from(format!("{}.corrupt-{stamp}", path.display()));
    let _ = tokio::fs::rename(path, &target).await;
}

/// 隔离文件名的时间戳（TS 用 `new Date().toISOString()`；Rust 侧同形 UTC）。
fn chrono_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    // 只求文件名唯一与可读；与 TS 的精确等价在阶段 3 需要时再对齐（隔离文件仅诊断用）。
    format!("epoch-{secs}-{millis:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_file_reads_the_typescript_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings = read(dir.path()).await.unwrap();
        assert_eq!(settings, default_settings());
        assert_eq!(settings["locale"], "zh-CN");
        assert_eq!(settings["localePreference"], "system");
        assert_eq!(settings["terminalInheritSystemProfile"], true);
        assert_eq!(settings["recentProjects"], json!([]));
    }

    #[tokio::test]
    async fn patch_merges_top_level_and_preserves_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"appearance":{"theme":"dark"},"recentProjects":["/a"],"legacyTeamConnection":{"id":"t1"}}"#,
        )
        .unwrap();
        let merged = apply_patch(
            dir.path(),
            &json!({"appearance": {"theme": "light"}, "language": "zh-CN"}),
        )
        .await
        .unwrap();
        // 补丁键覆盖、其它键原样保留（含 TS 的 legacy 字段——互操作关键性质）。
        assert_eq!(merged["appearance"]["theme"], "light");
        assert_eq!(merged["language"], "zh-CN");
        assert_eq!(merged["recentProjects"], json!(["/a"]));
        assert_eq!(merged["legacyTeamConnection"]["id"], "t1");
        // 落盘内容与返回值一致，且是 2 空格缩进的 JSON（TS JSON.stringify(..., null, 2) 同形）。
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with("{\n  \""));
        let parsed: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(parsed, merged);
    }

    #[tokio::test]
    async fn corrupt_file_is_quarantined_and_reads_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ this is not json").unwrap();
        let settings = read(dir.path()).await.unwrap();
        assert_eq!(settings, default_settings());
        assert!(!path.exists(), "坏文件被移走");
        let quarantined: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "隔离文件保留为诊断资产");
    }

    #[tokio::test]
    async fn valid_json_that_is_not_an_object_is_not_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[1,2,3]").unwrap();
        assert_eq!(read(dir.path()).await.unwrap(), default_settings());
        assert!(path.exists(), "合法 JSON 不隔离（TS 同为 schema 失败回退）");
    }

    #[tokio::test]
    async fn reads_a_file_written_by_the_other_host_shape() {
        // TS Host 写出的完整对象（含缩进与全部 schema 键）必须能被原样读回。
        let dir = tempfile::tempdir().unwrap();
        let path = settings_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let ts_style = serde_json::to_string_pretty(&json!({
            "appearance": {"theme": "system"},
            "language": "en-US",
            "closeToTrayOnWindowsMigrationInitialized": true,
            "providerFamilyConnectionSelections": {}
        }))
        .unwrap();
        std::fs::write(&path, format!("{ts_style}\n")).unwrap();
        let settings = read(dir.path()).await.unwrap();
        assert_eq!(settings["appearance"]["theme"], "system");
        assert_eq!(settings["closeToTrayOnWindowsMigrationInitialized"], true);
        assert_eq!(settings["language"], "en-US", "文件里的值优先");
        assert_eq!(settings["locale"], "zh-CN", "缺省字段补 TS 默认值");
        assert_eq!(settings["terminalInheritSystemProfile"], true);
    }
}
