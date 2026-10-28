//! 动态工作流灰度的折叠规则（TS `shared/dynamic-workflow-feature.ts: resolveDynamicWorkflowClientConfig`）。
//! 优先级：环境覆盖 > 远端合法值 > `disabled`；功能提供时合法的用户选择取代提供值，否则选择不生效。

use serde_json::{Map, Value, json};

use crate::js_compat::js_trim;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Disabled,
    OnDemand,
    AlwaysOn,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::OnDemand => "onDemand",
            Self::AlwaysOn => "alwaysOn",
        }
    }

    fn enabled(self) -> bool {
        self != Self::Disabled
    }
}

/// TS `normalizeDynamicWorkflowMode`：字符串，JS `trim()` 后精确匹配三个取值之一（区分大小写）。
pub fn normalize_mode(value: Option<&Value>) -> Option<Mode> {
    match js_trim(value?.as_str()?) {
        "disabled" => Some(Mode::Disabled),
        "onDemand" => Some(Mode::OnDemand),
        "alwaysOn" => Some(Mode::AlwaysOn),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Remote,
    Override,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicWorkflowConfig {
    pub mode: Mode,
    pub source: Source,
    pub offered_mode: Mode,
    pub user_mode: Option<Mode>,
}

impl DynamicWorkflowConfig {
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("mode".into(), json!(self.mode.as_str()));
        object.insert("enabled".into(), json!(self.mode.enabled()));
        object.insert(
            "source".into(),
            json!(match self.source {
                Source::Remote => "remote",
                Source::Override => "override",
                Source::Default => "default",
            }),
        );
        object.insert("offeredMode".into(), json!(self.offered_mode.as_str()));
        if let Some(user) = self.user_mode {
            object.insert("userMode".into(), json!(user.as_str()));
        }
        Value::Object(object)
    }
}

/// 「提供」：覆盖 > 远端合法值 > `disabled`。`remote` 取 `configs.dynamicWorkflow`（对象才读 `mode`）。
fn offer(remote: Option<&Value>, env_override: Option<&Value>) -> (Mode, Source) {
    if let Some(mode) = normalize_mode(env_override) {
        return (mode, Source::Override);
    }
    let remote_mode = remote
        .and_then(Value::as_object)
        .and_then(|object| object.get("mode"));
    match normalize_mode(remote_mode) {
        Some(mode) => (mode, Source::Remote),
        None => (Mode::Disabled, Source::Default),
    }
}

/// TS `resolveDynamicWorkflowClientConfig`。
pub fn resolve(
    remote: Option<&Value>,
    env_override: Option<&Value>,
    user_mode: Option<&Value>,
) -> DynamicWorkflowConfig {
    let (offered_mode, source) = offer(remote, env_override);
    // 上游 v3.16.2 起「功能已全量」：合法用户选择永远取代提供值（提供值只决定默认与设置页标签）。
    let choice = normalize_mode(user_mode);
    DynamicWorkflowConfig {
        mode: choice.unwrap_or(offered_mode),
        source,
        offered_mode,
        user_mode: choice,
    }
}
