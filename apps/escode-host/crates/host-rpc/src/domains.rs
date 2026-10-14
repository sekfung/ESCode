//! 已迁移的服务域：channels 表在这里登记（rust-u2-host-skeleton.md §5 优先序）。
//! 未登记的 channel 由 `session::respond` 回显式 `NotImplemented`。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::session::{ChannelHandler, ChannelResult, Channels};
use crate::wire::Wire;

/// `settingService`（TS `ServiceChannels.Setting` = `"setting"`）：
/// `get` 读设置文件、`update` 顶层合并补丁后原子写（与 Node Host 同文件同格式）。
/// 其余方法（`updateDataBaseDir` / `ensureDefaultProject`）回显式缺口。
pub fn setting_channel(home: std::path::PathBuf) -> (&'static str, ChannelHandler) {
    let handler: ChannelHandler = Arc::new(move |method: String, arg: Wire| {
        let home = home.clone();
        Box::pin(async move {
            match method.as_str() {
                "get" => escode_host_services::settings::read(&home)
                    .await
                    .map(Wire::Object)
                    .map_err(|error| ("Fatal".to_owned(), format!("{error:#}"))),
                "update" => {
                    // `ProxyChannel.toService` 把实参打包成数组：`update(patch)` → `[patch]`。
                    let patch = arg
                        .into_args()
                        .into_iter()
                        .next()
                        .and_then(Wire::into_object)
                        .ok_or_else(|| {
                            (
                                "InvalidRequest".to_owned(),
                                "setting.update requires a patch object".to_owned(),
                            )
                        })?;
                    escode_host_services::settings::apply_patch(&home, &patch)
                        .await
                        .map(|_| Wire::Undefined)
                        .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))
                }
                other => Err((
                    "NotImplemented".to_owned(),
                    format!("Host does not implement `setting.{other}` yet"),
                )),
            }
        })
    });
    ("setting", handler)
}

/// 从服务级参数里抽出运行时路由信息，并构造 CLI 形状的请求参数。
///
/// - 路由键（workspacePath / workspaceIdentity / remoteSessionId / dataDir / config / runtimePolicy）
///   只用于 Host 侧建实例，不转发；
/// - `workspace` 引用按 TS `buildWorkspaceRef` 构造（运行时用它做 workspace 校验）；
/// - 其余字段原样转发，`null` 一律省略（运行时的可选字段解析严格拒绝 null）。
struct RuntimeTarget {
    identity: String,
    descriptor: escode_host_control::WorkspaceDescriptor,
    request: Value,
}

fn runtime_target(params: Value) -> Result<RuntimeTarget, (String, String)> {
    let object = params.as_object().cloned().unwrap_or_default();
    let workspace_ref = object.get("workspace").cloned().unwrap_or(Value::Null);
    let pick = |key: &str| {
        object
            .get(key)
            .filter(|value| !value.is_null())
            .cloned()
            .or_else(|| {
                workspace_ref
                    .get(key)
                    .filter(|value| !value.is_null())
                    .cloned()
            })
    };
    let path = pick("workspacePath")
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    if path.is_empty() {
        return Err((
            "InvalidRequest".to_owned(),
            "escode-session calls require workspacePath".to_owned(),
        ));
    }
    let identity = pick("workspaceIdentity")
        .and_then(|value| value.as_str().map(str::trim).map(str::to_owned))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| path.clone());
    let remote_session_id =
        pick("remoteSessionId").and_then(|value| value.as_str().map(str::to_owned));
    let descriptor = escode_host_control::WorkspaceDescriptor {
        path: path.clone(),
        identity: Some(identity.clone()),
        data_dir: pick("dataDir").and_then(|value| value.as_str().map(str::to_owned)),
        config: pick("config").and_then(|value| value.as_str().map(str::to_owned)),
    };
    let mut request = object;
    for key in [
        "workspacePath",
        "workspaceIdentity",
        "remoteSessionId",
        "workspaceKey",
        "dataDir",
        "config",
        "runtimePolicy",
        "workspace",
    ] {
        request.remove(key);
    }
    // 可选字段缺省即省略（运行时 workspace 引用解析严格拒绝 null）。
    let mut workspace = json!({
        "workspacePath": path,
        "workspaceIdentity": identity,
        "workspaceKey": identity,
    });
    if let Some(remote_session_id) = remote_session_id {
        workspace["remoteSessionId"] = Value::String(remote_session_id);
    }
    request.insert("workspace".to_owned(), workspace);
    Ok(RuntimeTarget {
        identity,
        descriptor,
        request: Value::Object(request),
    })
}

/// 应答抽取：`Result` = 整个 `result`；`Field` = `result.<field>`。
#[derive(Clone, Copy)]
enum Extract {
    Result,
    Field(&'static str),
}

/// 把一次服务调用转发给该 workspace 的 runtime 实例。
async fn runtime_call(
    instances: &escode_host_runtime::Instances,
    options: &escode_host_surface::OptionsFactory,
    method: &str,
    params: Value,
    extract: Extract,
) -> ChannelResult {
    let target = runtime_target(params)?;
    let instance = instances
        .get(&target.identity, || options(&target.descriptor))
        .await
        .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))?;
    let answer = instance
        .call(method, target.request, std::time::Duration::from_secs(30))
        .await
        .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))?;
    if let Some(error) = answer.get("error") {
        return Err((
            "RuntimeError".to_owned(),
            error["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| error.to_string()),
        ));
    }
    let result = answer.get("result").cloned().ok_or_else(|| {
        (
            "Fatal".to_owned(),
            format!("runtime `{method}` answer lacks `result`: {answer}"),
        )
    })?;
    match extract {
        Extract::Result => Ok(Wire::Object(result)),
        Extract::Field(field) => result.get(field).cloned().map(Wire::Object).ok_or_else(|| {
            (
                "Fatal".to_owned(),
                format!("runtime `{method}` answer lacks `result.{field}`: {result}"),
            )
        }),
    }
}

/// `escode-session`（TS `ServiceChannels.ESCodeSession` = `"escode-session"`）：
/// TS Host 该域是对 Agent 协议的直接转发（`result` / `result.<field>` 原样返回），
/// Rust Host 同构——按 params 的 workspace 建/复用实例后转发。
/// 未迁移的方法（createSession / closeSession / setModel / …）回显式缺口。
pub fn session_channel(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> (&'static str, ChannelHandler) {
    let handler: ChannelHandler = Arc::new(move |method: String, arg: Wire| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        Box::pin(async move {
            let params = arg
                .into_args()
                .into_iter()
                .next()
                .and_then(Wire::into_object)
                .unwrap_or(Value::Null);
            match method.as_str() {
                "listSessions" => {
                    runtime_call(
                        &instances,
                        &options,
                        "session/list",
                        params,
                        Extract::Field("sessions"),
                    )
                    .await
                }
                "readSession" => {
                    runtime_call(
                        &instances,
                        &options,
                        "session/read",
                        params,
                        Extract::Result,
                    )
                    .await
                }
                "readSessionMessages" => {
                    runtime_call(
                        &instances,
                        &options,
                        "session/messages",
                        params,
                        Extract::Field("messages"),
                    )
                    .await
                }
                other => Err((
                    "NotImplemented".to_owned(),
                    format!("Host does not implement `escode-session.{other}` yet"),
                )),
            }
        })
    });
    ("escode-session", handler)
}

/// 组合根登记全部已迁移的 channel（Host 进程一处装配）。
pub fn channels(
    home: std::path::PathBuf,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channels {
    Channels::new(vec![
        setting_channel(home),
        session_channel(instances, options),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{
        REQUEST_PROMISE, REQUEST_PROMISE_CANCEL, RESPONSE_INITIALIZE, RESPONSE_PROMISE_ERROR,
        RESPONSE_PROMISE_SUCCESS, initialize_frame, parse_request, respond,
    };
    use crate::wire::{Reader, Writer};

    fn test_channels(home: std::path::PathBuf) -> Channels {
        let options: escode_host_surface::OptionsFactory =
            std::sync::Arc::new(|workspace: &escode_host_control::WorkspaceDescriptor| {
                escode_cli_rust::embed::RuntimeOptions {
                    home: std::path::PathBuf::from("/tmp"),
                    env: std::collections::BTreeMap::new(),
                    argv: Vec::new(),
                    cwd: std::path::PathBuf::from(&workspace.path),
                    workspace_identity: workspace.identity.clone(),
                    data_dir: None,
                    config: None,
                }
            });
        channels(
            home,
            escode_host_runtime::Instances::new(escode_host_runtime::Link::Embedded),
            options,
        )
    }

    fn request_fixture(kind: i64, id: i64, channel: &str, method: &str, arg: &Wire) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.value(&Wire::Array(vec![
            Wire::Int(kind),
            Wire::Int(id),
            Wire::Str(channel.to_owned()),
            Wire::Str(method.to_owned()),
        ]));
        writer.value(arg);
        writer.into_bytes()
    }

    #[tokio::test]
    async fn unknown_channel_reports_an_explicit_gap() {
        let channels = test_channels(std::path::PathBuf::from("/nonexistent"));
        let body = request_fixture(REQUEST_PROMISE, 9, "workspace", "query", &Wire::Undefined);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let header = reader.value().unwrap();
        let payload = reader.value().unwrap();
        let Wire::Array(items) = header else {
            panic!("response header must be an array")
        };
        assert_eq!(items[0].as_int(), Some(RESPONSE_PROMISE_ERROR));
        assert_eq!(items[1].as_int(), Some(9));
        let Wire::Object(error) = payload else {
            panic!("error payload must be an object")
        };
        assert_eq!(error["name"], "NotImplemented");
        assert!(error["message"].as_str().unwrap().contains("workspace"));
        assert!(error["message"].as_str().unwrap().contains("setting"));
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("escode-session"),
            "已实现清单应包含 escode-session: {error}"
        );
    }

    #[tokio::test]
    async fn settings_get_and_update_round_trip_over_the_wire() {
        let dir = tempfile::tempdir().unwrap();
        let channels = test_channels(dir.path().to_path_buf());
        let body = request_fixture(REQUEST_PROMISE, 3, "setting", "get", &Wire::Undefined);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let Wire::Array(header) = reader.value().unwrap() else {
            panic!("header")
        };
        assert_eq!(header[0].as_int(), Some(RESPONSE_PROMISE_SUCCESS));
        assert_eq!(reader.value().unwrap(), Wire::Object(json!({})));

        // 与 ProxyChannel.toService 同形：实参打包成数组。
        let patch = Wire::Array(vec![Wire::Object(json!({"locale": "zh-CN"}))]);
        let body = request_fixture(REQUEST_PROMISE, 4, "setting", "update", &patch);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let Wire::Array(header) = reader.value().unwrap() else {
            panic!("header")
        };
        assert_eq!(header[0].as_int(), Some(RESPONSE_PROMISE_SUCCESS));
        assert_eq!(reader.value().unwrap(), Wire::Undefined);
        // 落盘可由下一次 get 读到（同文件同格式）。
        let body = request_fixture(REQUEST_PROMISE, 5, "setting", "get", &Wire::Undefined);
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        let mut reader = Reader::new(&response);
        let _ = reader.value().unwrap();
        let Wire::Object(settings) = reader.value().unwrap() else {
            panic!("settings object")
        };
        assert_eq!(settings["locale"], "zh-CN");
    }

    #[tokio::test]
    async fn cancel_requests_have_no_response() {
        let channels = test_channels(std::path::PathBuf::from("/nonexistent"));
        let body = request_fixture(
            REQUEST_PROMISE_CANCEL,
            1,
            "setting",
            "get",
            &Wire::Undefined,
        );
        let response = respond(&channels, &parse_request(&body).unwrap()).await;
        assert!(response.is_empty());
    }

    #[test]
    fn initialize_frame_uses_response_type_200() {
        let frame = initialize_frame();
        let mut reader = Reader::new(&frame);
        let Wire::Array(header) = reader.value().unwrap() else {
            panic!("header")
        };
        assert_eq!(header, vec![Wire::Int(RESPONSE_INITIALIZE)]);
        assert_eq!(reader.value().unwrap(), Wire::Undefined);
    }
}
