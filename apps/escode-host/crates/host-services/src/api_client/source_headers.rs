//! 来源头（TS `providers/sourceHeaders.ts` + `shared/escode-source-headers.ts`）。
//! Host 没有 `Intl`/`os.version()`：locale、时区、OS 版本由 Main 经 `runtimeEnv` 下发（spec §3）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub const DEFAULT_REFERER: &str = "https://zcode.z.ai";

/// TS `normalizeESCodeSourceHeaderValue`：trim 后必须全是可打印 ASCII，否则视为缺省。
pub fn normalize_value(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty() && trimmed.bytes().all(|byte| (0x20..=0x7e).contains(&byte)))
        .then(|| trimmed.to_owned())
}

/// 进程里不变的客户端事实。
#[derive(Default)]
pub struct ClientIdentity {
    pub app_version: Option<String>,
    pub release_channel: Option<String>,
    pub language: Option<String>,
    pub timezone: Option<String>,
    pub os_version: Option<String>,
    /// `<appConfigDir>/telemetry-state.json`：只读复用已存在的 `deviceMid`，不生成新身份。
    pub device_state_file: Option<PathBuf>,
    device_mid: Mutex<Option<(PathBuf, String)>>,
}

impl ClientIdentity {
    pub fn from_env<'a>(
        get: impl Fn(&str) -> Option<&'a str>,
        app_config_dir: Option<PathBuf>,
    ) -> Self {
        let value = |name: &str| normalize_value(get(name));
        Self {
            app_version: value("ESCODE_APP_VERSION"),
            release_channel: value("ESCODE_ENV"),
            language: value("ESCODE_CLIENT_LANGUAGE"),
            timezone: value("ESCODE_CLIENT_TIMEZONE"),
            os_version: value("ESCODE_OS_VERSION"),
            device_state_file: app_config_dir.map(|dir| dir.join("telemetry-state.json")),
            device_mid: Mutex::default(),
        }
    }

    /// TS 请求上下文里的 `${process.platform}-${process.arch}`（如 `linux-x64`；与 P5 下载用的 `linux-x86_64` 不同）。
    pub fn platform_arch() -> String {
        format!("{}-{}", Self::node_platform(), Self::node_arch())
    }

    /// Node 的 `process.platform` 命名。
    fn node_platform() -> &'static str {
        match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        }
    }

    /// Node 的 `process.arch` 命名。
    fn node_arch() -> &'static str {
        match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            other => other,
        }
    }

    fn os_category(platform: &str) -> &'static str {
        match platform {
            "darwin" => "macos",
            "win32" => "windows",
            _ => "linux",
        }
    }

    /// TS `readExistingDeviceMid`：成功才缓存（失败下次再读），缓存键是文件路径。
    async fn device_mid(&self) -> Option<String> {
        let file = self.device_state_file.clone()?;
        if let Some((cached_file, value)) = self.device_mid.lock().ok()?.clone()
            && cached_file == file
        {
            return Some(value);
        }
        let text = tokio::fs::read_to_string(&file).await.ok()?;
        let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
        let value = normalize_value(parsed.get("deviceMid")?.as_str())?;
        *self.device_mid.lock().ok()? = Some((file, value.clone()));
        Some(value)
    }

    /// TS `buildESCodeSourceHeaders()`；键为小写。`HTTP-Referer` 恒为默认 Origin，是否改写由调用方按 Endpoint 决定。
    pub async fn headers(&self) -> BTreeMap<String, String> {
        let platform = Self::node_platform();
        let arch = Self::node_arch();
        let mut headers = BTreeMap::new();
        headers.insert("http-referer".to_owned(), DEFAULT_REFERER.to_owned());
        headers.insert(
            "user-agent".to_owned(),
            format!(
                "ESCode/{}",
                self.app_version.as_deref().unwrap_or("unknown")
            ),
        );
        if let Some(version) = &self.app_version {
            headers.insert("x-zcode-app-version".to_owned(), version.clone());
        }
        headers.insert("x-title".to_owned(), "ESCode@electron".to_owned());
        headers.insert("x-platform".to_owned(), format!("{platform}-{arch}"));
        if let Some(channel) = &self.release_channel {
            headers.insert("x-release-channel".to_owned(), channel.clone());
        }
        headers.insert(
            "x-client-language".to_owned(),
            self.language
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
        );
        headers.insert(
            "x-client-timezone".to_owned(),
            self.timezone
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
        );
        headers.insert(
            "x-os-category".to_owned(),
            Self::os_category(platform).to_owned(),
        );
        if let Some(version) = &self.os_version {
            headers.insert("x-os-version".to_owned(), version.clone());
        }
        if let Some(mid) = self.device_mid().await {
            headers.insert("x-device-mid".to_owned(), mid);
        }
        headers
    }
}
