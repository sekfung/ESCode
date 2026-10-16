//! `onboarding-record` channel（docs/specs/rust-u2-user-config-services.md §6）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use escode_host_services::onboarding::OnboardingRecords;
use escode_host_services::tasks_index::TasksIndex;
use escode_host_services::tasks_index::query::TaskFilters;
use serde_json::Value;

use crate::channel::Channel;
use crate::domains_task_support::tasks_index_path;
use crate::session::{MethodHandler, RpcError, handler_rpc};
use crate::wire::Wire;

fn arg_value(arg: &Wire, index: usize) -> Value {
    let item = match arg {
        Wire::Array(items) => items.get(index),
        other if index == 0 => Some(other),
        _ => None,
    };
    match item {
        Some(Wire::Str(text)) => Value::String(text.clone()),
        Some(Wire::Object(value)) => value.clone(),
        _ => Value::Null,
    }
}

/// TS `hasExistingLocalTask`：任务索引里至少有一条未删除任务。索引不可用按“没有”处理。
async fn has_existing_local_task(index_path: PathBuf) -> bool {
    tokio::task::spawn_blocking(move || {
        TasksIndex::open(&index_path)
            .and_then(|index| index.list_task_metas(&TaskFilters::default()))
            .is_ok_and(|metas| !metas.is_empty())
    })
    .await
    .unwrap_or(false)
}

fn method<F, Fut>(store: &Option<Arc<OnboardingRecords>>, run: F) -> MethodHandler
where
    F: Fn(Arc<OnboardingRecords>, Wire) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Wire, String>> + Send + 'static,
{
    let store = store.clone();
    let run = Arc::new(run);
    handler_rpc(move |arg: Wire| {
        let (store, run) = (store.clone(), Arc::clone(&run));
        Box::pin(async move {
            let Some(store) = store else {
                return Err(RpcError::new(
                    "NotImplemented",
                    "onboarding-record needs dataBaseDir: Main must pass dataBaseDir in rpcListen.",
                ));
            };
            run(store, arg)
                .await
                .map_err(|message| RpcError::new("Error", message))
        })
    })
}

fn nullable(value: Option<Value>) -> Wire {
    Wire::Object(value.unwrap_or(Value::Null))
}

pub(crate) fn onboarding_channel(data_base_dir: Option<&Path>) -> Channel {
    let store =
        data_base_dir.map(|base| Arc::new(OnboardingRecords::new(OnboardingRecords::path(base))));
    let index_path = tasks_index_path(data_base_dir.unwrap_or_else(|| Path::new("")));
    Channel::new(
        "onboarding-record",
        vec![
            (
                "appendRecord",
                method(&store, |s, arg| async move {
                    let device = arg_value(&arg, 0);
                    s.append_record(device.as_str().unwrap_or_default(), &arg_value(&arg, 1))
                        .await
                        .map(|()| Wire::Undefined)
                }),
            ),
            (
                "shouldOnboard",
                method(&store, move |s, arg| {
                    let index_path = index_path.clone();
                    async move {
                        let device = arg_value(&arg, 0);
                        let decision = s
                            .should_onboard(
                                device.as_str().unwrap_or_default(),
                                has_existing_local_task(index_path),
                            )
                            .await?;
                        Ok(Wire::Object(Value::Bool(decision)))
                    }
                }),
            ),
            (
                "dismissOnboarding",
                method(&store, |s, arg| async move {
                    let device = arg_value(&arg, 0);
                    s.dismiss(device.as_str().unwrap_or_default())
                        .await
                        .map(|()| Wire::Undefined)
                }),
            ),
            // userId 恒为 null：TS 对匿名用户是幂等空操作。
            (
                "claimAnonymousRecord",
                method(&store, |_s, _arg| async move { Ok(Wire::Undefined) }),
            ),
            (
                "getLatestEntry",
                method(&store, |s, _arg| async move {
                    Ok(nullable(s.latest_entry().await))
                }),
            ),
            (
                "syncSettingsFromRecord",
                method(&store, |s, _arg| async move {
                    Ok(nullable(s.settings_patch().await))
                }),
            ),
            (
                "updateRecordPreferences",
                method(&store, |s, arg| async move {
                    s.update_preferences(&arg_value(&arg, 0))
                        .await
                        .map(|()| Wire::Undefined)
                }),
            ),
            (
                "getRecords",
                method(
                    &store,
                    |s, _arg| async move { Ok(nullable(s.records().await)) },
                ),
            ),
            (
                "clearRecords",
                method(&store, |s, _arg| async move {
                    s.clear().await.map(|()| Wire::Undefined)
                }),
            ),
        ],
    )
}
