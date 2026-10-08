//! 配置文件的 `network` 段（docs/specs/rust-net-proxy.md「配置文件 `network` 段」）。
//!
//! TS `createConfig` 按 user < project < env 合并出 `network.httpProxy` / `noProxy` / `caCertFile`。修复：Rust
//! 原先只认环境变量，手写在配置文件里的代理与证书被忽略，同一份配置下两个 runtime 的出口不同。
//!
//! Node app-server 里不同出口读的是不同的合并视图（已用 App 差分实测），这里按同样的作用域对齐：
//! - `Process`：模型 provider registry 与 MCP 连接池在进程启动时 `createConfig({ env })`，不带工作区
//!   → 只有 user + env；
//! - `Workspace`：会话 app 的 WebFetch（httpClientPort）与工具子进程（executionPort）带 workingDirectory
//!   → user + project + env；
//! - `EnvOnly`：插件市场的下载与 git 子进程（`applyNetworkEgressEnv` 不传 network）→ 只有环境变量。
//!
//! 所有者：进程级 `OnceLock`，`main` 在建任何 HTTP 客户端前写入一次；Rust 不改写自身进程环境（多线程下
//! `set_var` 不安全）。有效值 = 同名环境变量（非空）?? 该作用域的文件值，与 TS 的 env 层覆盖文件层一致。
use serde_json::Value;
use std::sync::OnceLock;
use zcode_cli_domain::net_proxy::ProxyOptions;

pub const HTTP_PROXY_ENV_KEY: &str = "ZCODE_HTTP_PROXY";
pub const NO_PROXY_ENV_KEY: &str = "ZCODE_NO_PROXY";

/// 出口读哪一个合并视图（见模块注释）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkScope {
    EnvOnly,
    Process,
    Workspace,
}

/// 一个合并视图里的 `network` 段；只取非空字符串值。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkFileConfig {
    pub http_proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub ca_cert_file: Option<String>,
}

impl NetworkFileConfig {
    /// 从合并后的配置对象读 `network` 段。
    pub fn from_config(config: &Value) -> Self {
        let field = |key: &str| {
            config["network"][key]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        Self {
            http_proxy: field("httpProxy"),
            no_proxy: field("noProxy"),
            ca_cert_file: field("caCertFile"),
        }
    }

    /// 文件值换成与之等价的环境变量键（env 层缺席时才生效）。
    fn env_equivalents(&self) -> [(&'static str, Option<&String>); 3] {
        [
            (HTTP_PROXY_ENV_KEY, self.http_proxy.as_ref()),
            (NO_PROXY_ENV_KEY, self.no_proxy.as_ref()),
            (crate::tls_ca::EXPLICIT_CA_ENV_KEY, self.ca_cert_file.as_ref()),
        ]
    }
}

/// 两个合并视图：`user`（`~/.zcode/cli/config.json`）与 `workspace`（user 叠加项目层）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkFileLayers {
    pub user: NetworkFileConfig,
    pub workspace: NetworkFileConfig,
}

static FILE: OnceLock<NetworkFileLayers> = OnceLock::new();
static EMPTY: NetworkFileConfig = NetworkFileConfig {
    http_proxy: None,
    no_proxy: None,
    ca_cert_file: None,
};

/// 进程启动时写入一次；重复调用保留第一次的值。
pub fn install(layers: NetworkFileLayers) {
    let _ = FILE.set(layers);
}

fn file(scope: NetworkScope) -> &'static NetworkFileConfig {
    match (scope, FILE.get()) {
        (NetworkScope::Process, Some(layers)) => &layers.user,
        (NetworkScope::Workspace, Some(layers)) => &layers.workspace,
        _ => &EMPTY,
    }
}

/// 代理判定输入：显式值 = 环境变量 ?? 该作用域的文件值（TS 合并后的 `network.*`）。
pub fn proxy_options(scope: NetworkScope) -> ProxyOptions {
    options_from(file(scope), std::env::vars().collect())
}

fn options_from(file: &NetworkFileConfig, env: Vec<(String, String)>) -> ProxyOptions {
    let explicit = |key: &str, fallback: &Option<String>| {
        env.iter()
            .find(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v.clone())
            .or_else(|| fallback.clone())
    };
    ProxyOptions {
        http_proxy: explicit(HTTP_PROXY_ENV_KEY, &file.http_proxy),
        no_proxy: explicit(NO_PROXY_ENV_KEY, &file.no_proxy),
        env,
    }
}

/// 显式 CA 路径：`ZCODE_AGENT_CA_CERT` ?? 该作用域的 `network.caCertFile`（TS `resolveTlsCaCertFile`）。
pub(crate) fn explicit_ca_cert_file(scope: NetworkScope) -> Option<String> {
    std::env::var(crate::tls_ca::EXPLICIT_CA_ENV_KEY)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| file(scope).ca_cert_file.clone())
}

/// 工具子进程（Workspace 作用域，TS executionPort 的 network）差量计算用的环境视图：真实环境 + 环境里
/// 缺席的文件值（以等价的 `ZCODE_*` 键表示）。只用于计算差量，等价键本身不会因此写进子进程
/// （`runtime_env::child_env` 只据它们写标准代理 / CA 键）。
pub(crate) fn with_file_fallback(vars: Vec<(String, String)>) -> Vec<(String, String)> {
    with_fallback_from(file(NetworkScope::Workspace), vars, cfg!(windows))
}

fn with_fallback_from(
    file: &NetworkFileConfig,
    mut vars: Vec<(String, String)>,
    windows: bool,
) -> Vec<(String, String)> {
    for (key, value) in file.env_equivalents() {
        let Some(value) = value else { continue };
        let present = vars.iter().any(|(k, v)| {
            !v.is_empty()
                && if windows {
                    k.eq_ignore_ascii_case(key)
                } else {
                    k == key
                }
        });
        if !present {
            vars.push((key.to_owned(), value.clone()));
        }
    }
    vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file_config() -> NetworkFileConfig {
        NetworkFileConfig::from_config(&json!({
            "network": {"httpProxy": "http://file:1", "noProxy": "", "caCertFile": "/file.pem", "timeout": 5}
        }))
    }

    #[test]
    fn reads_only_non_empty_strings() {
        assert_eq!(
            file_config(),
            NetworkFileConfig {
                http_proxy: Some("http://file:1".into()),
                no_proxy: None,
                ca_cert_file: Some("/file.pem".into()),
            }
        );
        assert_eq!(
            NetworkFileConfig::from_config(&json!({})),
            NetworkFileConfig::default()
        );
    }

    #[test]
    fn environment_wins_over_file_and_empty_env_is_absent() {
        let env = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let options = options_from(&file_config(), env(&[]));
        assert_eq!(options.http_proxy.as_deref(), Some("http://file:1"));
        assert_eq!(options.no_proxy, None);
        let options = options_from(
            &file_config(),
            env(&[
                (HTTP_PROXY_ENV_KEY, "http://env:2"),
                (NO_PROXY_ENV_KEY, "localhost"),
            ]),
        );
        assert_eq!(options.http_proxy.as_deref(), Some("http://env:2"));
        assert_eq!(options.no_proxy.as_deref(), Some("localhost"));
        let options = options_from(&file_config(), env(&[(HTTP_PROXY_ENV_KEY, "")]));
        assert_eq!(options.http_proxy.as_deref(), Some("http://file:1"));
    }

    #[test]
    fn child_env_view_adds_missing_file_values() {
        let vars = with_fallback_from(
            &file_config(),
            vec![("zcode_agent_ca_cert".into(), "/env.pem".into())],
            true,
        );
        // Windows 键大小写不敏感：环境里已有 CA，不再追加文件值；代理缺席，按文件值补齐。
        assert_eq!(
            vars,
            vec![
                ("zcode_agent_ca_cert".to_owned(), "/env.pem".to_owned()),
                (HTTP_PROXY_ENV_KEY.to_owned(), "http://file:1".to_owned()),
            ]
        );
        let vars = with_fallback_from(&file_config(), vec![], false);
        assert!(vars.contains(&(
            crate::tls_ca::EXPLICIT_CA_ENV_KEY.to_owned(),
            "/file.pem".to_owned()
        )));
    }

    #[test]
    fn env_only_scope_never_reads_files() {
        // install 是进程级一次性写入，这里只验证未安装 / EnvOnly 时回空视图。
        assert_eq!(file(NetworkScope::EnvOnly), &EMPTY);
    }
}
