//! `svn` 域（docs/svn/architecture.md；TS 真相源 `packages/services/src/svn/`）。
//!
//! 与 Git 相同：svn CLI 是唯一事实源，服务无状态；可预期的业务失败以
//! `{ ok: false, issue, message }` 返回，进程级错误才抛 `anyhow::Error`。
//! 同一 workspace 身份 key 的写操作在 Host 内串行。

mod command;
mod commit_message;
mod config;
mod errors;
mod externals;
mod graph;
mod history;
mod parse;
mod paths;
mod reads;
mod runtime;
mod service;
mod shelf;
mod writes;
mod writes_lock;
mod writes_merge;
mod xml;

use std::time::Duration;

use serde_json::Value;

use self::config::{COMMAND_TIMEOUT, OUTPUT_BYTES};
pub use self::paths::BranchRef;
pub use self::runtime::SvnService;

#[derive(Clone, Debug)]
pub(crate) struct Credentials {
    pub username: String,
    pub password: String,
    pub store_in_cache: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedBinary {
    pub binary_path: String,
    pub version: String,
    pub supports_password_from_stdin: bool,
}

pub(crate) enum BinaryState {
    Ready(ResolvedBinary),
    NotFound,
    Unsupported {
        version: String,
        binary_path: String,
    },
}

pub(crate) struct RunRequest<'a> {
    pub cwd: &'a str,
    pub args: Vec<String>,
    pub credentials: Option<Credentials>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl<'a> RunRequest<'a> {
    pub fn new(cwd: &'a str, args: Vec<String>) -> Self {
        Self {
            cwd,
            args,
            credentials: None,
            timeout: COMMAND_TIMEOUT,
            max_output_bytes: OUTPUT_BYTES,
        }
    }

    pub fn credentials(mut self, credentials: Option<Credentials>) -> Self {
        self.credentials = credentials;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_output(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }
}

pub(crate) fn text(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn strings(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn credentials(params: &Value) -> Option<Credentials> {
    let credentials = params.get("credentials")?;
    let username = credentials.get("username")?.as_str()?.to_owned();
    let password = credentials
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Some(Credentials {
        username,
        password,
        store_in_cache: credentials
            .get("storeInSvnCache")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

pub(crate) fn workspace_key(params: &Value) -> String {
    params
        .get("workspaceIdentity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| text(params, "workspacePath"))
}

pub(crate) use parse::StatusEntry;

#[cfg(test)]
#[path = "svn_tests.rs"]
mod tests;
