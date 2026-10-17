//! `gen-ui` 域（docs/specs/rust-u2-gen-ui.md）：模型写可视化文件、Host 受限读取，以及 widget 状态存储。
//!
//! 四个状态成员与 `readDocument` 全是 **Host 本地能力**（不经运行时）。`prepareSandbox` 不在本域——
//! 它要在 Electron Main 注册 webview 沙箱，判定见 `rust-u2-host-scope-boundary.md` §3.2。

pub mod document;
pub mod paths;
pub mod schema;
pub mod state;

use std::path::Path;

use serde_json::Value;
use tokio::sync::mpsc;

use state::GenUiStateStore;

/// Host 内唯一的 `gen-ui` 服务（状态存储 + 输出根）。
#[derive(Clone)]
pub struct GenUiService {
    store: GenUiStateStore,
    output_root: String,
}

impl GenUiService {
    /// `data_base_dir` = Main 经 `rpcListen` 提交的应用数据根。
    pub fn new(data_base_dir: &Path) -> Self {
        Self {
            store: GenUiStateStore::new(paths::state_root(data_base_dir)),
            output_root: paths::output_root(data_base_dir)
                .to_string_lossy()
                .into_owned(),
        }
    }

    /// TS `readDocument` → `{path, html}`。
    pub fn read_document(&self, raw: &Value) -> Result<Value, String> {
        let document = document::read_document(raw, &self.output_root)?;
        Ok(serde_json::json!({"path": document.path, "html": document.html}))
    }

    /// TS `getState` → 状态或 `null`。
    pub async fn get_state(&self, raw: &Value) -> Result<Value, String> {
        self.store.get(raw).await
    }

    /// TS `setState`（入参 `{target, state}`）→ `undefined`。
    pub async fn set_state(&self, raw: &Value) -> Result<(), String> {
        self.store.set(raw).await
    }

    /// TS `listState` → `[{path, state}]`。
    pub async fn list_state(&self, raw: &Value) -> Result<Vec<Value>, String> {
        self.store.list(raw).await
    }

    /// TS `onStateChanged`（进程级广播，事件载荷 `{target, state}`）。
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Value> {
        self.store.subscribe()
    }
}
