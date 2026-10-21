//! MCP App 实例凭证的单一 owner（docs/specs/rust-mcp-ui.md §2）。
//!
//! 凭证只在 runtime 内铸造；Host 原样保存与回传。逻辑页面键相同的同代 open 幂等，
//! 连接代际变化即撤销旧凭证；撤销先从登记表移除再 abort，迟到结果无法重新获得执行权。
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// 凭证失效统一映射为 JSON-RPC -32000（docs/specs/rust-mcp-ui.md §3）。
fn unauthorized() -> anyhow::Error {
    crate::contract::RpcFailure::new(-32000, "MCP App instance is no longer authorized").into()
}

/// MCP 连接来源：规范化身份 + 单调代际。身份相同但代际不同视为换了连接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) identity: String,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) workspace: String,
    pub(crate) account_context: Option<String>,
    pub(crate) session_id: String,
    pub(crate) plugin_id: String,
    pub(crate) server_name: String,
    pub(crate) scope_id: String,
    pub(crate) resource_uri: String,
    pub(crate) owner_web_contents_id: u64,
}

impl Binding {
    fn logical_key(&self) -> String {
        json!([
            self.workspace,
            self.session_id,
            self.plugin_id,
            self.server_name,
            self.scope_id,
            self.resource_uri,
            self.owner_web_contents_id,
        ])
        .to_string()
    }

    fn app_identity(&self, source: &Source) -> String {
        let mut digest = Sha256::new();
        digest.update(
            json!([
                self.workspace,
                self.server_name,
                source.identity,
                self.account_context.as_deref().unwrap_or(""),
            ])
            .to_string(),
        );
        format!("{:x}", digest.finalize())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lease {
    pub(crate) runtime_id: String,
    pub(crate) generation: u64,
    pub(crate) token: String,
    pub(crate) app_identity: String,
}

impl Lease {
    pub(crate) fn to_value(&self) -> Value {
        json!({
            "runtimeId": self.runtime_id,
            "generation": self.generation,
            "token": self.token,
            "appIdentity": self.app_identity,
        })
    }
}

struct Record {
    lease: Lease,
    binding: Binding,
    source: Source,
    cancel: CancellationToken,
    pins: BTreeSet<String>,
}

/// 页面作用域校验参数（协议 `escodeMcpUiPluginScopeShape`）。
pub(crate) struct Scope<'a> {
    pub(crate) workspace: Option<&'a str>,
    pub(crate) session_id: &'a str,
    pub(crate) plugin_id: &'a str,
    pub(crate) server_name: &'a str,
}

pub(crate) struct Instances {
    runtime_id: String,
    generation: u64,
    records: BTreeMap<String, Record>,
    logical: BTreeMap<String, String>,
}

impl Default for Instances {
    fn default() -> Self {
        Self {
            runtime_id: uuid::Uuid::new_v4().to_string(),
            generation: 0,
            records: BTreeMap::new(),
            logical: BTreeMap::new(),
        }
    }
}

impl Instances {
    pub(crate) fn open(&mut self, binding: Binding, source: Source) -> Lease {
        let key = binding.logical_key();
        let previous = self
            .logical
            .get(&key)
            .and_then(|token| self.records.get(token))
            .map(|record| (record.lease.clone(), record.source.clone()));
        if let Some((lease, previous_source)) = previous.as_ref()
            && *previous_source == source
            && self
                .records
                .get(&lease.token)
                .is_some_and(|record| record.binding.account_context == binding.account_context)
        {
            return lease.clone();
        }
        if let Some((lease, _)) = previous.as_ref() {
            self.close(lease);
        }
        self.generation += 1;
        let lease = Lease {
            runtime_id: self.runtime_id.clone(),
            generation: self.generation,
            token: uuid::Uuid::new_v4().to_string(),
            app_identity: binding.app_identity(&source),
        };
        self.records.insert(
            lease.token.clone(),
            Record {
                lease: lease.clone(),
                binding,
                source,
                cancel: CancellationToken::new(),
                pins: BTreeSet::new(),
            },
        );
        self.logical.insert(key, lease.token.clone());
        lease
    }

    /// 凭证 + 归属 + 来源代际三重校验；来源失效即撤销。
    pub(crate) fn validate(
        &mut self,
        lease: &Lease,
        scope: &Scope<'_>,
        source: Option<&Source>,
    ) -> Result<CancellationToken> {
        let Some(record) = self.records.get(&lease.token) else {
            return Err(unauthorized());
        };
        if record.lease.runtime_id != lease.runtime_id
            || record.lease.generation != lease.generation
            || record.lease.app_identity != lease.app_identity
            || scope
                .workspace
                .is_some_and(|workspace| record.binding.workspace != workspace)
            || record.binding.session_id != scope.session_id
            || record.binding.plugin_id != scope.plugin_id
            || record.binding.server_name != scope.server_name
        {
            return Err(unauthorized());
        }
        let current = source.is_some_and(|source| {
            source.identity == record.source.identity
                && source.generation == record.source.generation
        });
        if !current {
            let invalid = lease.clone();
            self.close(&invalid);
            return Err(unauthorized());
        }
        Ok(self
            .records
            .get(&lease.token)
            .map(|record| record.cancel.clone())
            .unwrap_or_default())
    }

    pub(crate) fn retain(&mut self, lease: &Lease, call_id: &str) -> Result<Pin> {
        let Some(record) = self.records.get_mut(&lease.token) else {
            return Err(unauthorized());
        };
        if record.lease.generation != lease.generation {
            return Err(unauthorized());
        }
        let pin = format!("{call_id}:{}", uuid::Uuid::new_v4());
        record.pins.insert(pin.clone());
        Ok(Pin {
            token: lease.token.clone(),
            pin,
        })
    }

    pub(crate) fn release(&mut self, pin: &Pin) {
        if let Some(record) = self.records.get_mut(&pin.token) {
            record.pins.remove(&pin.pin);
        }
    }

    pub(crate) fn is_busy(&self, lease: &Lease) -> bool {
        self.records
            .get(&lease.token)
            .is_some_and(|record| !record.pins.is_empty())
    }

    /// `mcp/readResource` 的绑定校验：只允许读实例绑定的那份资源。
    pub(crate) fn resource_uri(&self, lease: &Lease) -> Option<String> {
        self.records
            .get(&lease.token)
            .map(|record| record.binding.resource_uri.clone())
    }

    /// 撤销凭证；返回是否本来就有效。登记表的取消令牌随之 abort。
    pub(crate) fn close(&mut self, lease: &Lease) -> bool {
        let Some(record) = self.records.get(&lease.token) else {
            return false;
        };
        if record.lease.runtime_id != lease.runtime_id
            || record.lease.generation != lease.generation
            || record.lease.app_identity != lease.app_identity
        {
            return false;
        }
        let key = record.binding.logical_key();
        let cancel = record.cancel.clone();
        self.records.remove(&lease.token);
        if self.logical.get(&key) == Some(&lease.token) {
            self.logical.remove(&key);
        }
        cancel.cancel();
        true
    }

    /// 关闭允许连接已经断开，但不能用另一来源或会话的凭证撤销当前实例（TS `closeMcpUiInstance`）。
    pub(crate) fn close_in_scope(&mut self, lease: &Lease, scope: &Scope<'_>) -> bool {
        let matches = self.records.get(&lease.token).is_some_and(|record| {
            record.lease.runtime_id == lease.runtime_id
                && record.lease.generation == lease.generation
                && record.lease.app_identity == lease.app_identity
                && scope
                    .workspace
                    .is_none_or(|workspace| record.binding.workspace == workspace)
                && record.binding.session_id == scope.session_id
                && record.binding.plugin_id == scope.plugin_id
                && record.binding.server_name == scope.server_name
        });
        matches && self.close(lease)
    }

    pub(crate) fn close_session(&mut self, session_id: &str) {
        let leases: Vec<Lease> = self
            .records
            .values()
            .filter(|record| record.binding.session_id == session_id)
            .map(|record| record.lease.clone())
            .collect();
        for lease in leases {
            self.close(&lease);
        }
    }
}

/// 调用 pin：drop 不自动释放，必须经 `Instances::release`。
pub(crate) struct Pin {
    token: String,
    pin: String,
}
