//! `gen-ui` channel（TS `ServiceChannels.GenUi` = `"gen-ui"`；docs/specs/rust-u2-gen-ui.md）。
//!
//! `readDocument` 是同步受限读（走 `spawn_blocking_io`），四个状态成员是异步文件操作，
//! `onStateChanged` 是进程级广播事件。`prepareSandbox` 的壳 HTML 与初始状态由 Host 组装，
//! 最后的 webview 注册经控制通道交给 Electron Main（边界子 spec §3.2）。

use std::sync::Arc;

use escode_host_control::main_broker::MainBroker;
use escode_host_services::gen_ui::GenUiService;
use futures_util::stream;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domains_file::spawn_blocking_io;
use crate::domains_task_support::first_object;
use crate::session::{Channel, EventStream, handler, handler_rpc};
use crate::wire::Wire;

/// TS `GEN_UI_HTML_MAX_BYTES`。
const GEN_UI_HTML_MAX_BYTES: usize = 5 * 1024 * 1024;
/// TS `GEN_UI_STATIC_DOMAINS`。
const GEN_UI_STATIC_DOMAINS: [&str; 7] = [
    "cdnjs.cloudflare.com",
    "esm.sh",
    "cdn.jsdelivr.net",
    "unpkg.com",
    "fonts.googleapis.com",
    "fonts.gstatic.com",
    "fonts.bunny.net",
];

fn failed(error: String) -> (String, String) {
    ("Error".to_owned(), error)
}

/// 缺 `dataBaseDir` 时方法表照常登记（契约对账要求），调用一律回结构化缺口。
fn gap_channel() -> Channel {
    let gap = "Host has no dataBaseDir: Main must pass dataBaseDir in rpcListen.";
    let missing = || {
        handler(move |_arg: Wire| {
            let gap = gap.to_owned();
            Box::pin(async move { Err(("InvalidRequest".to_owned(), gap)) })
        })
    };
    Channel::new(
        "gen-ui",
        vec![
            ("readDocument", missing()),
            ("getState", missing()),
            ("setState", missing()),
            ("listState", missing()),
            ("prepareSandbox", missing()),
        ],
    )
    .with_events(vec![(
        "onStateChanged",
        Arc::new(|_arg: Wire| Ok(Box::pin(stream::pending()) as EventStream)),
    )])
}

pub(crate) fn gen_ui_channel(
    data_base_dir: Option<std::path::PathBuf>,
    main_broker: Option<Arc<MainBroker>>,
) -> Channel {
    let Some(data_base_dir) = data_base_dir else {
        return gap_channel();
    };
    let service = Arc::new(GenUiService::new(&data_base_dir));

    let read_document = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let document =
                    spawn_blocking_io(move || service.read_document(&params).map_err(failed))
                        .await?;
                Ok::<Wire, (String, String)>(Wire::Object(document))
            })
        })
    };
    let get_state = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let state = service.get_state(&params).await.map_err(failed)?;
                Ok(Wire::Object(state))
            })
        })
    };
    let set_state = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                service.set_state(&params).await.map_err(failed)?;
                Ok(Wire::Undefined)
            })
        })
    };
    let list_state = {
        let service = Arc::clone(&service);
        handler(move |arg: Wire| {
            let (service, params) = (Arc::clone(&service), first_object(arg));
            Box::pin(async move {
                let entries = service.list_state(&params).await.map_err(failed)?;
                Ok::<Wire, (String, String)>(Wire::Array(
                    entries.into_iter().map(Wire::Object).collect(),
                ))
            })
        })
    };
    // 进程级广播：每个订阅者一条独立流；无静态事件参数。
    let change = {
        let service = Arc::clone(&service);
        Arc::new(move |_arg: Wire| {
            let receiver = service.subscribe();
            let stream: EventStream =
                Box::pin(stream::unfold(receiver, |mut receiver| async move {
                    receiver
                        .recv()
                        .await
                        .map(|value: Value| (Wire::Object(value), receiver))
                }));
            Ok(stream)
        })
    };

    let prepare = {
        let service = Arc::clone(&service);
        let broker = main_broker;
        let runtime_id = format!("gen-ui:{}", Uuid::new_v4());
        handler_rpc(move |arg: Wire| {
            let service = Arc::clone(&service);
            let broker = broker.clone();
            let runtime_id = runtime_id.clone();
            let params = first_object(arg);
            Box::pin(async move { prepare_sandbox(service, broker, runtime_id, &params).await })
        })
    };

    Channel::new(
        "gen-ui",
        vec![
            ("readDocument", read_document),
            ("getState", get_state),
            ("setState", set_state),
            ("listState", list_state),
            ("prepareSandbox", prepare),
        ],
    )
    .with_events(vec![("onStateChanged", change)])
}

async fn prepare_sandbox(
    service: Arc<GenUiService>,
    broker: Option<Arc<MainBroker>>,
    runtime_id: String,
    params: &Value,
) -> crate::session::RpcResult {
    use crate::session::RpcError;
    let Some(broker) = broker else {
        return Err(RpcError::new(
            "NotImplemented",
            "gen-ui.prepareSandbox requires Electron Main; Main did not declare mainServices.",
        ));
    };
    let workspace_path = params["workspacePath"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::new("InvalidRequest", "workspacePath is required."))?;
    let session_id = params["sessionId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::new("InvalidRequest", "sessionId is required."))?;
    let path = params["path"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| RpcError::new("InvalidRequest", "path is required."))?;
    let html = params["html"]
        .as_str()
        .ok_or_else(|| RpcError::new("InvalidRequest", "html is required."))?;
    let owner = params["ownerWebContentsId"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            RpcError::new(
                "InvalidRequest",
                "ownerWebContentsId must be a positive integer.",
            )
        })?;
    let instance_key = params["instanceKey"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 1024)
        .ok_or_else(|| RpcError::new("InvalidRequest", "instanceKey is required."))?;
    assert_gen_ui_fragment(html).map_err(|message| RpcError::new("InvalidRequest", message))?;

    let identity = params["workspaceIdentity"].as_str().map(str::trim);
    let scope_key = json!([
        identity
            .filter(|value| !value.is_empty())
            .unwrap_or(workspace_path),
        session_id
    ])
    .to_string();
    let app_identity = sha256_hex(json!(["gen-ui", scope_key, path]).to_string().as_bytes());
    let mut target = json!({
        "workspacePath": workspace_path,
        "sessionId": session_id,
        "path": path,
    });
    if let Some(identity) = identity.filter(|value| !value.is_empty()) {
        target["workspaceIdentity"] = Value::from(identity);
    }
    let state = service.get_state(&target).await.map_err(failed)?;
    let initial = serde_json::to_string(&state)
        .unwrap_or_else(|_| "null".to_owned())
        .replace('<', "\\u003c");
    let full_html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><!--__ESCODE_GEN_UI_STYLES__--><script type=\"application/json\" id=\"escode-gen-ui-initial-state\">{initial}</script><script src=\"__ESCODE_GEN_UI_RUNTIME__\"></script></head><body>{html}<!--__ESCODE_GEN_UI_INNER_KIT__--></body></html>"
    );
    let resource_domains: Vec<String> = GEN_UI_STATIC_DOMAINS
        .iter()
        .map(|host| format!("https://{host}"))
        .collect();
    let mut register = json!({
        "contentKind": "gen-ui",
        "instance": {
            "appIdentity": app_identity,
            "runtimeId": runtime_id,
            "generation": 1,
            "token": Uuid::new_v4().to_string(),
        },
        "ownerWebContentsId": owner,
        "workspacePath": workspace_path,
        "sessionId": session_id,
        "scopeId": instance_key,
        "html": full_html,
        "csp": {
            "connectDomains": [],
            "resourceDomains": resource_domains,
            "frameDomains": [],
            "baseUriDomains": [],
        },
    });
    if let Some(identity) = identity.filter(|value| !value.is_empty()) {
        register["workspaceIdentity"] = Value::from(identity);
    }
    broker
        .request("gen-ui.prepareSandbox", register)
        .await
        .map(Wire::Object)
        .map_err(|message| RpcError::new("Error", message))
}

fn assert_gen_ui_fragment(html: &str) -> Result<(), String> {
    if html.len() > GEN_UI_HTML_MAX_BYTES {
        return Err("Gen UI file exceeds 5 MB".to_owned());
    }
    let lower = html.to_ascii_lowercase();
    for tag in [
        "<!doctype",
        "<html",
        "</html",
        "<head",
        "</head",
        "<body",
        "</body",
    ] {
        let mut offset = 0;
        while let Some(found) = lower[offset..].find(tag) {
            let end = offset + found + tag.len();
            let boundary = lower[end..]
                .chars()
                .next()
                .is_none_or(|ch| ch.is_whitespace() || ch == '>');
            if boundary {
                return Err("Gen UI requires an HTML fragment".to_owned());
            }
            offset = end;
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
