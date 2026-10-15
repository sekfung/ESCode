//! `provider-settings` 的写入面 RPC（docs/specs/rust-u2-providers.md §8）。
//!
//! 实参是 `ProxyChannel.toService` 打包的数组；缺省实参（`undefined`）折成 `Value::Null`。
//! 返回值与 TS 同形：多数方法回最新 `ProviderSettingsView`，`createPersonalProvider` 回 `{providerId, view}`。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use escode_host_services::provider::ProviderService;
use serde_json::Value;

use crate::session::{MethodHandler, handler};
use crate::wire::Wire;

type Service = Option<Arc<ProviderService>>;
type Output = Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send>>;

fn to_value(wire: Wire) -> Value {
    match wire {
        Wire::Undefined => Value::Null,
        Wire::Str(text) => Value::String(text),
        Wire::Int(number) => Value::from(number),
        Wire::Object(value) => value,
        Wire::Array(items) => Value::Array(items.into_iter().map(to_value).collect()),
        Wire::Bytes { .. } => Value::Null,
    }
}

/// 位置实参的取值器。
struct Args(Vec<Value>);

impl Args {
    fn text(&self, index: usize) -> String {
        self.0
            .get(index)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    fn value(&self, index: usize) -> Value {
        self.0.get(index).cloned().unwrap_or(Value::Null)
    }

    /// `undefined`/`null` 都视为未提供。
    fn optional(&self, index: usize) -> Option<Value> {
        self.0.get(index).filter(|value| !value.is_null()).cloned()
    }

    fn flag(&self, index: usize) -> Option<bool> {
        self.0.get(index).and_then(Value::as_bool)
    }
}

fn method(
    service: Service,
    call: impl Fn(Arc<ProviderService>, Args) -> Output + Send + Sync + 'static,
) -> MethodHandler {
    let call = Arc::new(call);
    handler(move |arg: Wire| {
        let service = service.clone();
        let call = Arc::clone(&call);
        Box::pin(async move {
            let Some(service) = service else {
                return Err(super::domains_provider::missing_files());
            };
            let args = Args(arg.into_args().into_iter().map(to_value).collect());
            call(service, args)
                .await
                .map(Wire::Object)
                .map_err(super::domains_provider::error)
        })
    })
}

/// 写入面的全部方法（名称与 `IProviderSettingsService` 一致）。
pub(crate) fn write_methods(service: &Service) -> Vec<(&'static str, MethodHandler)> {
    let m = |call: fn(Arc<ProviderService>, Args) -> Output| method(service.clone(), call);
    vec![
        (
            "createPersonalProvider",
            m(|s, a| {
                Box::pin(async move { s.create_personal_provider(a.optional(0).as_ref()).await })
            }),
        ),
        (
            "savePersonalProviderOverlay",
            m(|s, a| {
                Box::pin(async move {
                    s.save_personal_provider_overlay(
                        &a.text(0),
                        &a.value(1),
                        a.optional(2).as_ref(),
                    )
                    .await
                })
            }),
        ),
        (
            "deletePersonalProvider",
            m(|s, a| Box::pin(async move { s.delete_personal_provider(&a.text(0)).await })),
        ),
        (
            "reorderPersonalProviders",
            m(|s, a| Box::pin(async move { s.reorder_personal_providers(&a.value(0)).await })),
        ),
        (
            "reorderPersonalModels",
            m(|s, a| {
                Box::pin(async move { s.reorder_personal_models(&a.text(0), &a.value(1)).await })
            }),
        ),
        (
            "addPersonalModel",
            m(|s, a| {
                Box::pin(async move {
                    s.add_personal_model(&a.text(0), &a.text(1), &a.value(2), a.flag(3))
                        .await
                })
            }),
        ),
        (
            "renamePersonalModel",
            m(|s, a| {
                Box::pin(async move {
                    s.rename_personal_model(&a.text(0), &a.text(1), &a.text(2))
                        .await
                })
            }),
        ),
        (
            "deletePersonalModel",
            m(|s, a| {
                Box::pin(async move { s.delete_personal_model(&a.text(0), &a.text(1)).await })
            }),
        ),
        (
            "savePersonalModelDraft",
            m(|s, a| Box::pin(async move { s.save_personal_model_draft(&a.value(0)).await })),
        ),
        (
            "setPersonalModelEnabled",
            m(|s, a| {
                Box::pin(async move {
                    s.set_personal_model_enabled(&a.text(0), &a.text(1), &a.value(2))
                        .await
                })
            }),
        ),
    ]
}
