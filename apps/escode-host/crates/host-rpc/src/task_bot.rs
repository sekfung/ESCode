//! Bot topic 的 Host 侧 admission（TS `createBotTopicAdmission`）。
//!
//! `invalidate` 先推进版本、再等待在途发送收口；`submit` 在准备（快照/附件）后
//! 重新校验版本，保证停止请求不会让旧输入在准备完成后才发出。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as AsyncMutex;

pub(crate) struct BotGroupState {
    entries: Mutex<HashMap<String, Entry>>,
    /// Host 内部 bot 连接身份（V4 订阅/附件事务共用；进程内稳定）。
    pub(crate) connection_id: String,
}

impl Default for BotGroupState {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            connection_id: format!("host-task-bot-{}", uuid::Uuid::new_v4()),
        }
    }
}

struct Entry {
    version: u64,
    lock: Arc<AsyncMutex<()>>,
}

impl BotGroupState {
    pub(crate) fn capture(&self, key: &str) -> u64 {
        self.lock().get(key).map_or(0, |entry| entry.version)
    }

    pub(crate) fn begin(&self, key: &str) -> (u64, Arc<AsyncMutex<()>>) {
        let mut entries = self.lock();
        let entry = entries.entry(key.to_owned()).or_insert_with(|| Entry {
            version: 0,
            lock: Arc::new(AsyncMutex::new(())),
        });
        (entry.version, Arc::clone(&entry.lock))
    }

    pub(crate) fn is_current(&self, key: &str, version: u64) -> bool {
        self.capture(key) == version
    }

    pub(crate) async fn invalidate(&self, key: &str) {
        let lock = {
            let mut entries = self.lock();
            let entry = entries.entry(key.to_owned()).or_insert_with(|| Entry {
                version: 0,
                lock: Arc::new(AsyncMutex::new(())),
            });
            entry.version += 1;
            Arc::clone(&entry.lock)
        };
        // 先推进版本（让准备中的输入失败），再等待已发出的命令收口。
        let _guard = lock.lock().await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
#[path = "task_bot_tests.rs"]
mod tests;
