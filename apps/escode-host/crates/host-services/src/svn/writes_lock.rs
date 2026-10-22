//! svn lock / unlock（TS `svnWrites.lock/unlock`）。

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::config::{REMOTE_READ_TIMEOUT, normalize_svn_path};
use super::errors::failure;
use super::paths::{is_safe_relative_path, local_target};
use super::{RunRequest, SvnService, credentials, text, workspace_key};

pub(crate) async fn lock(service: &SvnService, params: &Value) -> Value {
    let mut seen = BTreeSet::new();
    let paths: Vec<String> = super::strings(params, "paths")
        .into_iter()
        .map(|path| normalize_svn_path(&path))
        .filter(|path| seen.insert(path.clone()))
        .collect();
    if paths.is_empty() || !paths.iter().all(|path| is_safe_relative_path(path)) {
        return failure("invalid-request", "paths must be relative to the workspace");
    }
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let mut args = vec!["lock".to_owned()];
            let message = text(params, "message");
            if !message.trim().is_empty() {
                args.push("-m".to_owned());
                args.push(message.trim().to_owned());
            }
            args.push("--".to_owned());
            args.extend(paths.iter().map(|path| local_target(path)));
            match service
                .run(
                    RunRequest::new(&text(params, "workspacePath"), args)
                        .credentials(credentials(params))
                        .timeout(REMOTE_READ_TIMEOUT),
                )
                .await
            {
                Ok(_) => json!({ "ok": true }),
                Err(error) => error,
            }
        })
        .await
}

pub(crate) async fn unlock(service: &SvnService, params: &Value) -> Value {
    let mut seen = BTreeSet::new();
    let paths: Vec<String> = super::strings(params, "paths")
        .into_iter()
        .map(|path| normalize_svn_path(&path))
        .filter(|path| seen.insert(path.clone()))
        .collect();
    if paths.is_empty() || !paths.iter().all(|path| is_safe_relative_path(path)) {
        return failure("invalid-request", "paths must be relative to the workspace");
    }
    let force = params
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let key = workspace_key(params);
    service
        .enqueue_write(&key, || async {
            let mut args = vec!["unlock".to_owned()];
            if force {
                args.push("--force".to_owned());
            }
            args.push("--".to_owned());
            args.extend(paths.iter().map(|path| local_target(path)));
            match service
                .run(
                    RunRequest::new(&text(params, "workspacePath"), args)
                        .credentials(credentials(params))
                        .timeout(REMOTE_READ_TIMEOUT),
                )
                .await
            {
                Ok(_) => json!({ "ok": true }),
                Err(error) => error,
            }
        })
        .await
}
