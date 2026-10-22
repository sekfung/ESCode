//! svn 服务运行时：二进制探测、命令执行、写队列与历史页缓存。
//!
//! 与 Git 相同，Host 内只有一个实例；`process_env` 由组合根经 `core_api::env` 取得。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use escode_host_control::logging;
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use super::config::{binary_candidates, command_env, compare_versions};
use super::errors;
use super::{BinaryState, ResolvedBinary, RunRequest, command};

/// Host 内唯一的 svn 服务（二进制探测缓存 + 写队列）。
pub struct SvnService {
    pub(super) env: BTreeMap<String, String>,
    pub(super) candidates: Vec<String>,
    pub(super) home: PathBuf,
    pub(super) temp_root: PathBuf,
    pub(super) binary_cache: Mutex<HashMap<String, ResolvedBinary>>,
    pub(super) write_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(super) log_cache: Mutex<Vec<(String, Value)>>,
}

impl SvnService {
    /// `process_env` 由组合根经 `core_api::env` 唯一出口取得；`home` = 设置域根。
    pub fn new(process_env: &[(String, String)], home: PathBuf, temp_root: PathBuf) -> Self {
        Self {
            env: command_env(process_env),
            candidates: binary_candidates(process_env, None),
            home,
            temp_root,
            binary_cache: Mutex::new(HashMap::new()),
            write_locks: Mutex::new(HashMap::new()),
            log_cache: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn temp_root(&self) -> &Path {
        &self.temp_root
    }

    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    pub(crate) async fn clear_binary_cache(&self) {
        self.binary_cache.lock().await.clear();
    }

    pub(crate) async fn cached_log(&self, key: &str) -> Option<Value> {
        self.log_cache
            .lock()
            .await
            .iter()
            .find(|(cached_key, _)| cached_key == key)
            .map(|(_, page)| page.clone())
    }

    pub(crate) async fn cache_log(&self, key: String, page: Value) {
        let mut cache = self.log_cache.lock().await;
        if !cache.iter().any(|(cached_key, _)| *cached_key == key) {
            if cache.len() >= super::config::LOG_CACHE_MAX_ENTRIES {
                cache.remove(0);
            }
            cache.push((key, page));
        }
    }

    fn configured_path(&self, settings: &Value) -> String {
        settings
            .get("svnBinaryPath")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_owned()
    }

    pub(crate) async fn resolve_binary(&self) -> BinaryState {
        let settings = crate::settings::read(&self.home)
            .await
            .unwrap_or_else(|_| Value::Object(Map::new()));
        let configured = self.configured_path(&settings);
        if let Some(cached) = self.binary_cache.lock().await.get(&configured) {
            return classify_binary(cached.clone());
        }
        let mut candidates: Vec<String> = Vec::new();
        if !configured.is_empty() {
            candidates.push(configured.clone());
        }
        for candidate in &self.candidates {
            if !candidates.contains(candidate) {
                candidates.push(candidate.clone());
            }
        }
        for candidate in candidates {
            if let Some(version) = command::probe_version(&candidate, &self.env).await {
                let binary = ResolvedBinary {
                    supports_password_from_stdin: compare_versions(&version, "1.10.0")
                        != std::cmp::Ordering::Less,
                    binary_path: candidate,
                    version,
                };
                self.binary_cache
                    .lock()
                    .await
                    .insert(configured, binary.clone());
                return classify_binary(binary);
            }
        }
        BinaryState::NotFound
    }

    /// 执行一次 svn 命令，完成凭据参数、超时与错误归类。
    pub(crate) async fn run(&self, request: RunRequest<'_>) -> Result<command::Output, Value> {
        let binary = match self.resolve_binary().await {
            BinaryState::Ready(binary) => binary,
            BinaryState::NotFound => {
                return Err(errors::failure(
                    "svn-unavailable",
                    "svn executable was not found",
                ));
            }
            BinaryState::Unsupported {
                version,
                binary_path: _,
            } => {
                return Err(errors::failure(
                    "svn-unavailable",
                    format!(
                        "svn {version} is older than {}",
                        super::config::MIN_SUPPORTED_VERSION
                    ),
                ));
            }
        };
        let mut args = vec!["--non-interactive".to_owned()];
        args.extend(request.args);
        let (args, stdin) = with_credentials(
            args,
            request.credentials,
            binary.supports_password_from_stdin,
        );
        let subcommand = args
            .iter()
            .find(|arg| !arg.starts_with('-'))
            .cloned()
            .unwrap_or_default();
        let output = command::run(
            &binary.binary_path,
            &self.env,
            command::RunOptions {
                cwd: request.cwd,
                args,
                stdin,
                timeout: request.timeout,
                max_output_bytes: request.max_output_bytes,
            },
        )
        .await;
        logging::debug(
            "svn",
            format!(
                "svn 命令完成 subcommand={subcommand} exit={:?} durationMs={} timedOut={}",
                output.exit_code, output.duration_ms, output.timed_out
            ),
        );
        if output.timed_out {
            return Err(errors::failure("timeout", "svn command timed out"));
        }
        if output.exit_code != Some(0) && !output.output_truncated {
            let issue = errors::classify(&output.stderr);
            if matches!(
                issue,
                "working-copy-locked" | "auth-required" | "out-of-date"
            ) {
                logging::warn(
                    "svn",
                    format!("svn 命令失败 subcommand={subcommand} issue={issue}"),
                );
            }
            let message = errors::summarize(&output.stderr);
            return Err(errors::failure(
                issue,
                if message.is_empty() {
                    "svn command failed".to_owned()
                } else {
                    message
                },
            ));
        }
        Ok(output)
    }

    async fn write_lock(&self, key: &str) -> Arc<Mutex<()>> {
        self.write_locks
            .lock()
            .await
            .entry(key.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// 同一 workspace 身份 key 的写操作串行；读操作不排队。
    pub(crate) async fn enqueue_write<F, Fut, T>(&self, key: &str, job: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let lock = self.write_lock(key).await;
        let _guard = lock.lock().await;
        job().await
    }
}

/// 凭据参数：svn ≥ 1.10 用 stdin 传密码，避免密码出现在进程列表；旧版本回退 `--password`。
pub(super) fn with_credentials(
    mut args: Vec<String>,
    credentials: Option<super::Credentials>,
    supports_password_from_stdin: bool,
) -> (Vec<String>, Option<String>) {
    let Some(credentials) = credentials else {
        return (args, None);
    };
    args.insert(0, "--username".to_owned());
    args.insert(1, credentials.username);
    let stdin = if supports_password_from_stdin {
        args.insert(0, "--password-from-stdin".to_owned());
        Some(credentials.password)
    } else {
        args.insert(0, "--password".to_owned());
        args.insert(1, credentials.password);
        None
    };
    if !credentials.store_in_cache {
        args.insert(0, "--no-auth-cache".to_owned());
    }
    (args, stdin)
}

fn classify_binary(binary: ResolvedBinary) -> BinaryState {
    if compare_versions(&binary.version, super::config::MIN_SUPPORTED_VERSION)
        == std::cmp::Ordering::Less
    {
        BinaryState::Unsupported {
            version: binary.version,
            binary_path: binary.binary_path,
        }
    } else {
        BinaryState::Ready(binary)
    }
}
