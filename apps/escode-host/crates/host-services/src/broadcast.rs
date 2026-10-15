//! `broadcast` 本地模式（docs/specs/rust-u2-user-config-services.md §3；TS `createBroadcastService(null)`）。

use std::sync::{Mutex, MutexGuard};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::git::paths::js_trim;

const RESERVATION_TTL_MS: u64 = 5_000;
const RETRY_MS: u64 = 250;
const MAX_LOCAL_CLAIMS: usize = 1_024;

#[derive(Clone, PartialEq, Eq, Debug)]
enum Status {
    Reserved,
    Committed,
}

#[derive(Clone, Debug)]
struct Claim {
    token: String,
    status: Status,
    expires_at: Option<u64>,
}

#[derive(Default)]
struct Claims {
    /// JS `Map` 的插入序：覆盖已有键保持原位置，删除后重加移到末尾。
    entries: Vec<(String, Claim)>,
}

impl Claims {
    fn get(&self, key: &str) -> Option<&Claim> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, c)| c)
    }

    fn set(&mut self, key: &str, claim: Claim) {
        match self.entries.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = claim,
            None => self.entries.push((key.to_owned(), claim)),
        }
    }

    fn delete(&mut self, key: &str) {
        self.entries.retain(|(k, _)| k != key);
    }

    /// TS `pruneLocalClaims`：先清过期 reservation，再按插入序淘汰超限的最早项。
    fn prune(&mut self, now: u64) {
        self.entries.retain(|(_, claim)| {
            !(claim.status == Status::Reserved && claim.expires_at.is_some_and(|at| at <= now))
        });
        while self.entries.len() > MAX_LOCAL_CLAIMS {
            self.entries.remove(0);
        }
    }
}

pub struct Broadcast {
    messages: broadcast::Sender<Value>,
    claims: Mutex<Claims>,
}

impl Default for Broadcast {
    fn default() -> Self {
        Self {
            messages: broadcast::channel(256).0,
            claims: Mutex::default(),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Broadcast {
    fn claims(&self) -> MutexGuard<'_, Claims> {
        self.claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 订阅消息流（每个 `onMessage` 监听一份接收端）。
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.messages.subscribe()
    }

    /// TS `broadcastMessageSchema.parse` + 本地 emitter：`channel` 去空白后非空，`sourceWindowId` 为整数。
    pub fn send(&self, message: &Value) -> Result<(), String> {
        let channel = message
            .get("channel")
            .and_then(Value::as_str)
            .map(js_trim)
            .filter(|channel| !channel.is_empty())
            .ok_or("Invalid broadcast message: channel must be a non-empty string")?;
        let mut validated = serde_json::Map::new();
        validated.insert("channel".into(), json!(channel));
        // zod 4 的 `z.unknown()` 不再是可缺省键：消息必须带 `payload`（可以是 null）。
        let payload = message
            .get("payload")
            .ok_or("Invalid broadcast message: payload is required")?;
        validated.insert("payload".into(), payload.clone());
        match message.get("sourceWindowId") {
            None | Some(Value::Null) => {}
            Some(id)
                if id.is_i64() || id.is_u64() || id.as_f64().is_some_and(|f| f.fract() == 0.0) =>
            {
                validated.insert("sourceWindowId".into(), id.clone());
            }
            Some(_) => {
                return Err("Invalid broadcast message: sourceWindowId must be an integer".into());
            }
        }
        // 没有订阅者时 send 返回 Err，不是错误。
        let _ = self.messages.send(Value::Object(validated));
        Ok(())
    }

    pub fn acquire_claim(&self, key: &str) -> Value {
        self.acquire_at(key, now_ms(), || format!("local-{}", uuid::Uuid::new_v4()))
    }

    fn acquire_at(&self, key: &str, now: u64, token: impl FnOnce() -> String) -> Value {
        let key = js_trim(key);
        if key.is_empty() {
            return json!({"status": "unavailable"});
        }
        let mut claims = self.claims();
        claims.prune(now);
        if let Some(existing) = claims.get(key) {
            if existing.status == Status::Committed {
                return json!({"status": "committed"});
            }
            let remaining = existing.expires_at.unwrap_or(now).saturating_sub(now);
            return json!({"status": "busy", "retryAfterMs": remaining.min(RETRY_MS)});
        }
        let token = token();
        claims.set(
            key,
            Claim {
                token: token.clone(),
                status: Status::Reserved,
                expires_at: Some(now + RESERVATION_TTL_MS),
            },
        );
        claims.prune(now);
        json!({"status": "acquired", "lease": {"key": key, "token": token}})
    }

    pub fn commit_claim(&self, key: &str, token: &str) {
        self.commit_at(key, token, now_ms());
    }

    fn commit_at(&self, key: &str, token: &str, now: u64) {
        let mut claims = self.claims();
        claims.prune(now);
        if let Some(current) = claims.get(key).cloned()
            && current.status == Status::Reserved
            && current.token == token
        {
            claims.set(
                key,
                Claim {
                    status: Status::Committed,
                    expires_at: None,
                    ..current
                },
            );
        }
    }

    pub fn release_claim(&self, key: &str, token: &str) {
        self.release_at(key, token, now_ms());
    }

    fn release_at(&self, key: &str, token: &str, now: u64) {
        let mut claims = self.claims();
        claims.prune(now);
        if claims
            .get(key)
            .is_some_and(|current| current.status == Status::Reserved && current.token == token)
        {
            claims.delete(key);
        }
    }

    pub fn try_claim(&self, key: &str) -> bool {
        let result = self.acquire_claim(key);
        if result["status"] != "acquired" {
            return false;
        }
        let lease = &result["lease"];
        self.commit_claim(
            lease["key"].as_str().unwrap_or_default(),
            lease["token"].as_str().unwrap_or_default(),
        );
        true
    }
}

#[cfg(test)]
#[path = "broadcast_tests.rs"]
mod tests;
