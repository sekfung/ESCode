//! 远端 scope 的未读信号派生（拆出 `remote.rs` 以守住 400 行门禁）。

use serde_json::Value;

use super::RemoteController;
use super::unread;

impl RemoteController {
    /// 远端事实的 owner 在 Source Server：这里只把同一份事件转成窗口级未读提示。
    pub(super) fn publish_unread(&self, event: &Value) {
        let Some(mut signal) = unread::signal_of(event) else {
            return;
        };
        if let Some(source_server_id) = &self.scope.source_server_id {
            signal["address"]["sourceServerId"] = Value::from(source_server_id.clone());
        }
        self.unread.emit(&signal);
    }
}
