//! `system.listIntegratedTerminalShells` / `probeIntranet`（docs/specs/rust-u2-user-config-services.md §5）。

use escode_host_services::system_probe;

use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

/// 环境读取走 `core_api::env`（嵌入实例下以实例 env 快照为准；宿主进程直读被边界门禁禁止）。
fn env_lookup(key: &str) -> Option<String> {
    escode_cli_core_api::env::var(key).ok()
}

/// Windows 上“可执行”= 文件存在（Node `accessSync(X_OK)` 在 Windows 只检查存在）；其它平台不会走到这里。
fn is_executable(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

pub(crate) fn extra_methods() -> Vec<(&'static str, MethodHandler)> {
    vec![
        (
            "listIntegratedTerminalShells",
            handler(|_arg: Wire| {
                Box::pin(async move {
                    let shells = tokio::task::spawn_blocking(|| {
                        system_probe::list_integrated_terminal_shells(
                            cfg!(windows),
                            &env_lookup,
                            &is_executable,
                        )
                    })
                    .await
                    .map_err(|error| ("Fatal".to_owned(), error.to_string()))?;
                    Ok(Wire::Object(serde_json::Value::Array(shells)))
                })
            }),
        ),
        (
            "probeIntranet",
            handler(|arg: Wire| {
                Box::pin(async move {
                    let request = arg
                        .into_args()
                        .into_iter()
                        .next()
                        .and_then(Wire::into_object)
                        .unwrap_or(serde_json::Value::Null);
                    system_probe::probe_intranet(&request)
                        .await
                        .map(Wire::Object)
                        .map_err(|message| ("Error".to_owned(), message))
                })
            }),
        ),
    ]
}
