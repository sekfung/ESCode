//! 已迁移的服务域：channels 表在这里登记（rust-u2-host-skeleton.md §5 优先序）。
//! 未登记的 channel 由 `session::respond` 回显式 `NotImplemented`。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::forward::{Extract, runtime_call, runtime_call_with};
use crate::session::{Channel, Channels, handler};
use crate::wire::Wire;

/// `settingService`（TS `ServiceChannels.Setting` = `"setting"`）：
/// `get` 读设置文件、`update` 顶层合并补丁后原子写（与 Node Host 同文件同格式）；
/// 其余方法（`updateDataBaseDir` / `ensureDefaultProject`）未实现——方法表即分派，
/// 缺口由 `respond` 回显式 `NotImplemented` 并列出已实现方法。
pub fn setting_channel(home: std::path::PathBuf) -> Channel {
    let read_home = home.clone();
    let write_home = home;
    Channel::new(
        "setting",
        vec![
            (
                "get",
                handler(move |_arg: Wire| {
                    let home = read_home.clone();
                    Box::pin(async move {
                        escode_host_services::settings::read(&home)
                            .await
                            .map(Wire::Object)
                            .map_err(|error| ("Fatal".to_owned(), format!("{error:#}")))
                    })
                }),
            ),
            (
                "update",
                handler(move |arg: Wire| {
                    let home = write_home.clone();
                    Box::pin(async move {
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
                    })
                }),
            ),
        ],
    )
}

/// `escode-session`（TS `ServiceChannels.ESCodeSession` = `"escode-session"`）：
/// TS Host 该域是对 Agent 协议的直接转发（`result` / `result.<field>` 原样返回），
/// Rust Host 同构——按 params 的 workspace 建/复用实例后转发。
/// 未迁移的方法回显式缺口：`createSession` / `resumeSession`（需 Host 侧特性门
/// offPeak/dynamicWorkflow 参数整形）、`closeDeferredDraftSession` / `promoteDeferredDraftSession`
/// （需 Host 侧 deferred-draft 注册表）、`readSessionEvents`（运行时无 `session/events`）、
/// 以及 workspace / provider 握手类方法（属各自域）。
pub fn session_channel(
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channel {
    // 每个方法一个表项：参数取用与服务级参数的转发形状在各方法内自洽。
    fn params_of(arg: Wire) -> Value {
        arg.into_args()
            .into_iter()
            .next()
            .and_then(Wire::into_object)
            .unwrap_or(Value::Null)
    }

    let forward = |runtime_method: &'static str, extract: Extract| {
        let instances = instances.clone();
        let options = Arc::clone(&options);
        handler(move |arg: Wire| {
            let instances = instances.clone();
            let options = Arc::clone(&options);
            Box::pin(async move {
                runtime_call(
                    &instances,
                    &options,
                    runtime_method,
                    params_of(arg),
                    extract,
                )
                .await
            })
        })
    };

    Channel::new(
        "escode-session",
        vec![
            (
                "listSessions",
                forward("session/list", Extract::Field("sessions")),
            ),
            ("readSession", forward("session/read", Extract::Result)),
            (
                "readSessionMessages",
                forward("session/messages", Extract::Field("messages")),
            ),
            (
                "closeSession",
                forward("session/close", Extract::FieldOr("closed", true)),
            ),
            ("setModel", forward("session/setModel", Extract::Result)),
            (
                "setThoughtLevel",
                forward("session/setThoughtLevel", Extract::Result),
            ),
            ("setMode", forward("session/setMode", Extract::Result)),
            (
                "initializeWorkspace",
                handler({
                    let instances = instances.clone();
                    let options = Arc::clone(&options);
                    move |arg: Wire| {
                        let instances = instances.clone();
                        let options = Arc::clone(&options);
                        Box::pin(async move {
                            // workspace 预热：TS `agentService.initialize` 返回 agent 可用性与协议常量，
                            // 首屏不因 agent 缺失崩溃（失败也回 `available:false`）。Rust Host 同构：
                            // 建/复用实例后等运行时报到；`transportKind` 固定为 "stdio"。
                            let target = crate::forward::runtime_target(params_of(arg))?;
                            let workspace_key = target.identity.clone();
                            let available = match instances
                                .get(&target.identity, || options(&target.descriptor))
                                .await
                            {
                                Ok(instance) => {
                                    instance
                                        .wait_ready(std::time::Duration::from_secs(30))
                                        .await
                                }
                                Err(_) => false,
                            };
                            Ok(Wire::Object(json!({
                                "available": available,
                                "workspaceKey": workspace_key,
                                "protocolName": "ESCode Protocol",
                                "protocolVersion": 1,
                                "transportKind": "stdio",
                            })))
                        })
                    }
                }),
            ),
            (
                "readWorkspacePresentation",
                handler({
                    let instances = instances.clone();
                    let options = Arc::clone(&options);
                    move |arg: Wire| {
                        let instances = instances.clone();
                        let options = Arc::clone(&options);
                        Box::pin(async move {
                            // workspace 展示面：TS agent 层先读 `runtime/capabilities` 再带
                            // `includeExecutionCapabilities` 读 `workspace/readPresentation`，这里同序转发。
                            // 已登记差异：TS 读取前会同步账号 provider 配置（providers/account 域未迁移）。
                            let params = params_of(arg);
                            let capabilities = runtime_call(
                                &instances,
                                &options,
                                "runtime/capabilities",
                                params.clone(),
                                Extract::Result,
                            )
                            .await?;
                            let Wire::Object(capabilities) = capabilities else {
                                return Err((
                                    "Fatal".to_owned(),
                                    "runtime/capabilities answer is not an object".to_owned(),
                                ));
                            };
                            let extra = (capabilities["workspaceExecutionCapabilities"] == true)
                                .then_some(("includeExecutionCapabilities", Value::Bool(true)));
                            runtime_call_with(
                                &instances,
                                &options,
                                "workspace/readPresentation",
                                params,
                                extra,
                                Extract::Result,
                            )
                            .await
                        })
                    }
                }),
            ),
        ],
    )
}

/// `system`（TS `ServiceChannels.System` = `"system"`）：`info()` 返回
/// `{homedir, platform}`（TS `os.homedir()` + `process.platform`；Main 经 `rpcListen`
/// 给出的 home 与 Host 进程 HOME 同源）。终端 shell 探测与内网探测未实现。
pub fn system_channel(home: std::path::PathBuf) -> Channel {
    Channel::new(
        "system",
        vec![(
            "info",
            handler(move |_arg: Wire| {
                let home = home.clone();
                Box::pin(async move {
                    Ok(Wire::Object(json!({
                        "homedir": home.to_string_lossy(),
                        "platform": node_platform(),
                    })))
                })
            }),
        )],
    )
}

/// `process.platform` 口径（TS 用 Node 名字：darwin / win32 / linux）。
fn node_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

/// 组合根登记全部已迁移的 channel（Host 进程一处装配）。
/// `home` = 设置域根（homedir 语义）；`data_base_dir` = Main 提交的应用数据根（会话工作区用）。
pub fn channels(
    home: std::path::PathBuf,
    data_base_dir: Option<std::path::PathBuf>,
    instances: escode_host_runtime::Instances,
    options: escode_host_surface::OptionsFactory,
) -> Channels {
    let home_for_file = home.clone();
    Channels::new(vec![
        setting_channel(home.clone()),
        system_channel(home),
        crate::domains_file_channel::file_channel(home_for_file, data_base_dir),
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
    use serde_json::json;

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
            home.clone(),
            Some(home),
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
