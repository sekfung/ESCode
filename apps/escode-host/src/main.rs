//! ESCode App 级 Rust Host 进程入口（docs/specs/rust-u2-host-skeleton.md §3.4/§3.5）。
//!
//! 职责（AGENTS.md 状态所有者：Host 拥有实例表、代次、订阅归属与调度）：
//! - Main 经 stdio 控制通道提交 `WindowAttach`（含 workspace 描述），Host 为该 workspace
//!   建/复用 runtime 实例（`host-runtime::Instances`：single-flight + 懒重启 generation）；
//! - Main 转发的 Renderer 请求行进入实例，实例出站帧按 `WindowFrame` 回给 Main；
//! - `Shutdown`：逐个实例 cancel → 宽限 drain → 残余清单 + 退出码。
//!
//! 本进程不承载业务状态，也不监听信号以外的事件源；RuntimeOptions 由控制消息提供，
//! 进程 env 只用于自身日志与 HOME 缺省（组合根职责，类似 CLI 的 main.rs）。

mod control;

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use escode_host_control::{ControlError, ControlErrorCode, ControlMessage};
use escode_host_runtime::{Instances, Link};

use crate::control::ControlLoop;

/// 协议版本（Main 握手用；不匹配时 Main 应回退 Node Host）。
const PROTOCOL_VERSION: u32 = 1;

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(code) => code,
        Err(error) => {
            // 启动期错误用结构化控制帧上抛，Main 据此决定重试或回退（不静默退出）。
            eprintln!("escode-host: {error:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32> {
    escode_cli_core_api::install_process_snapshot();
    let link = link_from_env();
    let instances = Instances::new(link);
    let mut control = ControlLoop::start().context("Host control channel unavailable")?;
    control
        .send(ControlMessage::HostReady {
            protocol_version: PROTOCOL_VERSION,
            workspaces: 0,
        })
        .await?;
    let code = serve(&mut control, &instances).await?;
    Ok(code)
}

/// runtime-link 选择器：默认 embedded；`ESCODE_HOST_RUNTIME_LINK=child` 时用 CLI 子进程
/// （差分、回退与故障隔离选项）；`ESCODE_HOST_RUNTIME_BINARY` 指定二进制路径。
fn link_from_env() -> Link {
    let child = std::env::var("ESCODE_HOST_RUNTIME_LINK").is_ok_and(|value| value == "child");
    if !child {
        return Link::Embedded;
    }
    let binary = std::env::var("ESCODE_HOST_RUNTIME_BINARY").map_or_else(
        |_| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("escode-cli-rust")))
                .unwrap_or_default()
        },
        std::path::PathBuf::from,
    );
    Link::Child { binary }
}

/// 控制循环：attach/detach/请求转发/关闭。
async fn serve(control: &mut ControlLoop, instances: &Instances) -> Result<i32> {
    // window → 实例身份（attachment 路由由 Host 拥有）。
    let mut attachments: BTreeMap<String, String> = BTreeMap::new();
    // Renderer 传输面（hello/NDJSON 版，供早期接入联调）。
    let mut surfaces: Vec<escode_host_surface::Surface> = Vec::new();
    // Renderer RPC 面（@escode/rpc 协议，Renderer 真实链路）。
    let mut rpc_surfaces: Vec<escode_host_rpc::RpcSurface> = Vec::new();
    while let Some(message) = control.recv().await {
        match message {
            ControlMessage::WindowAttach {
                window_id,
                client_id: _,
                workspace,
            } => {
                let identity = control::identity_of(&workspace);
                match instances
                    .get(&identity, || control::runtime_options(&workspace))
                    .await
                {
                    Ok(instance) => {
                        attachments.insert(window_id.clone(), identity.clone());
                        control
                            .send(ControlMessage::WindowAttached {
                                window_id: window_id.clone(),
                                generation: instance.generation,
                            })
                            .await?;
                        // 帧泵按实例一个，向全部 attachment 扇出（多窗口共享同一实例）。
                        control.attach(&identity, &window_id, instance);
                    }
                    Err(error) => {
                        let code = if error
                            .downcast_ref::<escode_cli_rust::adapters::storage::WorkspaceOwned>()
                            .is_some()
                        {
                            ControlErrorCode::WorkspaceOwned
                        } else {
                            ControlErrorCode::Fatal
                        };
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::WindowDetach { window_id } => {
                if let Some(identity) = attachments.remove(&window_id) {
                    control.detach(&identity, &window_id);
                }
                control
                    .send(ControlMessage::WindowDetached { window_id })
                    .await?;
            }
            ControlMessage::WindowRequest { window_id, line } => {
                let Some(identity) = attachments.get(&window_id).cloned() else {
                    control
                        .send(ControlMessage::Failed {
                            error: ControlError {
                                code: ControlErrorCode::NotReady,
                                message: format!("window {window_id} is not attached"),
                            },
                        })
                        .await?;
                    continue;
                };
                let instance = instances
                    .live(&identity)
                    .with_context(|| format!("workspace instance for {identity} is not available"));
                match instance {
                    Ok(instance) => {
                        if let Err(error) = instance.send_line(&line).await {
                            control
                                .send(ControlMessage::Failed {
                                    error: ControlError {
                                        code: ControlErrorCode::Fatal,
                                        message: format!("{error:#}"),
                                    },
                                })
                                .await?;
                        }
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::NotReady,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::RpcListen {
                bind,
                home,
                data_base_dir,
                runtime_env,
            } => {
                let addr: std::net::SocketAddr = bind
                    .parse()
                    .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
                let runtime_env = runtime_env.unwrap_or_default();
                // provider 配置文件路径：Main 合成的运行时环境里的两个约定变量（A16b 的 P1 部分）。
                let provider_files = match (
                    runtime_env.get("ESCODE_BUILTIN_PROVIDER_CONFIG_FILE"),
                    runtime_env.get("ESCODE_PERSONAL_PROVIDER_CONFIG_FILE"),
                ) {
                    (Some(builtin), Some(personal)) => Some((
                        std::path::PathBuf::from(builtin),
                        std::path::PathBuf::from(personal),
                    )),
                    _ => None,
                };
                let options: escode_host_surface::OptionsFactory =
                    std::sync::Arc::new(move |workspace| {
                        control::runtime_options_with(workspace, &runtime_env)
                    });
                let channels = escode_host_rpc::domains::channels_with(
                    std::path::PathBuf::from(home),
                    data_base_dir.map(std::path::PathBuf::from),
                    instances.clone(),
                    options,
                    provider_files,
                );
                match escode_host_rpc::serve(addr, channels, Some(instances.clone())).await {
                    Ok(surface) => {
                        control
                            .send(ControlMessage::RpcListening {
                                addr: surface.addr.to_string(),
                                secret: surface.secret.clone(),
                            })
                            .await?;
                        rpc_surfaces.push(surface);
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("rpc listen failed: {error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::SurfaceListen { bind } => {
                let addr: std::net::SocketAddr = bind
                    .parse()
                    .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], 0)));
                let options: escode_host_surface::OptionsFactory =
                    std::sync::Arc::new(control::runtime_options);
                match escode_host_surface::Surface::start(addr, instances.clone(), options).await {
                    Ok(surface) => {
                        control
                            .send(ControlMessage::SurfaceListening {
                                addr: surface.addr.to_string(),
                            })
                            .await?;
                        // 传输面随 Host 进程存活；句柄留在这里以便 Shutdown 时收口。
                        surfaces.push(surface);
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("surface listen failed: {error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::SettingsRead { home } => {
                let settings = escode_host_services::settings::read(std::path::Path::new(&home))
                    .await
                    .unwrap_or_else(|error| {
                        eprintln!("escode-host: settings read failed: {error:#}");
                        serde_json::json!({})
                    });
                control
                    .send(ControlMessage::SettingsValue { settings })
                    .await?;
            }
            ControlMessage::SettingsWrite { home, patch } => {
                match escode_host_services::settings::apply_patch(
                    std::path::Path::new(&home),
                    &patch,
                )
                .await
                {
                    Ok(settings) => {
                        control
                            .send(ControlMessage::SettingsValue { settings })
                            .await?;
                    }
                    Err(error) => {
                        control
                            .send(ControlMessage::Failed {
                                error: ControlError {
                                    code: ControlErrorCode::Fatal,
                                    message: format!("{error:#}"),
                                },
                            })
                            .await?;
                    }
                }
            }
            ControlMessage::Shutdown { reason } => {
                for surface in surfaces.drain(..) {
                    surface.shutdown().await;
                }
                for surface in rpc_surfaces.drain(..) {
                    surface.shutdown().await;
                }
                control.stop_pumps();
                let _ = control
                    .send(ControlMessage::LogBatch {
                        scope: "host".to_owned(),
                        level: escode_host_control::LogLevel::Info,
                        lines: vec![format!("shutdown: {reason}")],
                    })
                    .await;
                let residual = instances.shutdown().await;
                control
                    .send(ControlMessage::Stopped {
                        residual: residual.clone(),
                    })
                    .await?;
                // 残余任务/进程在宽限内没收回：非零退出，Main 记录并（阶段 3）回退。
                return Ok(if residual.is_empty() { 0 } else { 2 });
            }
            // Host→Main 的消息不应出现在入站方向；忽略以免回环。
            ControlMessage::HostReady { .. }
            | ControlMessage::WindowAttached { .. }
            | ControlMessage::WindowDetached { .. }
            | ControlMessage::WindowFrame { .. }
            | ControlMessage::LogBatch { .. }
            | ControlMessage::Failed { .. }
            | ControlMessage::Stopped { .. }
            | ControlMessage::SettingsValue { .. }
            | ControlMessage::SurfaceListening { .. }
            | ControlMessage::RpcListening { .. } => {}
        }
    }
    // 输入关闭（Main 退出）= 收口全部实例与传输面。
    for surface in surfaces.drain(..) {
        surface.shutdown().await;
    }
    for surface in rpc_surfaces.drain(..) {
        surface.shutdown().await;
    }
    control.stop_pumps();
    let residual = instances.shutdown().await;
    if residual.is_empty() {
        Ok(0)
    } else {
        eprintln!("escode-host: residual work at EOF: {}", residual.join(", "));
        Ok(2)
    }
}
