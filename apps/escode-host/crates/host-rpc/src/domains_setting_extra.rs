//! `setting.ensureDefaultProject` / `updateDataBaseDir`（docs/specs/rust-u2-user-config-services.md §4）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use escode_host_services::data_dir;
use escode_host_services::git::paths::js_trim;
use serde_json::{Value, json};

use crate::domains_file::node_error;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

fn first(arg: Wire) -> Wire {
    arg.into_args()
        .into_iter()
        .next()
        .unwrap_or(Wire::Undefined)
}

pub(crate) fn extra_methods(
    home: &Path,
    data_base_dir: Option<&Path>,
) -> Vec<(&'static str, MethodHandler)> {
    let ensure = handler_rpc(|arg: Wire| {
        Box::pin(async move {
            let Wire::Str(user_home) = first(arg) else {
                return Err(RpcError::new(
                    "InvalidRequest",
                    "ensureDefaultProject requires a home directory string",
                ));
            };
            let (path, created) = data_dir::ensure_default_project(Path::new(&user_home))
                .await
                .map_err(|e| RpcError::new("NodeIoError", node_error(&e, "mkdir", &user_home)))?;
            Ok(Wire::Object(
                json!({"path": path.to_string_lossy(), "created": created}),
            ))
        })
    });
    let home = home.to_path_buf();
    let current = data_base_dir.map_or_else(|| home.clone(), Path::to_path_buf);
    let update = handler_rpc({
        let (home, current) = (Arc::new(home), Arc::new(current));
        move |arg: Wire| {
            let (home, current) = (Arc::clone(&home), Arc::clone(&current));
            Box::pin(async move {
                let requested = match first(arg) {
                    Wire::Str(text) => Some(text),
                    _ => None,
                };
                // TS：`newDir?.trim() || homedir()`
                let target: PathBuf =
                    match requested.as_deref().map(js_trim).filter(|d| !d.is_empty()) {
                        Some(dir) => PathBuf::from(dir),
                        None => home.as_ref().clone(),
                    };
                let target_text = target.to_string_lossy().into_owned();
                let lookup = |key: &str| escode_cli_core_api::env::var(key).ok();
                if let Some(forbidden) =
                    data_dir::forbidden_install_dir(&target_text, cfg!(windows), &lookup)
                {
                    return Err(RpcError::new(
                        "Error",
                        format!("{}: {forbidden}", data_dir::FORBIDDEN_INSTALL_DIR_CODE),
                    )
                    .with("code", Value::from(data_dir::FORBIDDEN_INSTALL_DIR_CODE)));
                }
                if current.as_ref() != &target {
                    let (from, to) = (current.as_ref().clone(), target.clone());
                    tokio::task::spawn_blocking(move || data_dir::copy_data_directory(&from, &to))
                        .await
                        .map_err(|e| RpcError::new("Fatal", e.to_string()))?
                        .map_err(|e| {
                            RpcError::new("NodeIoError", node_error(&e, "cp", &target_text))
                        })?;
                }
                // `update({dataBaseDir: newDir})`：undefined 经 JSON 序列化即移除该键。
                escode_host_services::settings::set_key(
                    &home,
                    "dataBaseDir",
                    requested.map(Value::String),
                )
                .await
                .map_err(|e| RpcError::new("Fatal", format!("{e:#}")))?;
                Ok(Wire::Undefined)
            })
        }
    });
    vec![
        ("ensureDefaultProject", ensure),
        ("updateDataBaseDir", update),
    ]
}
