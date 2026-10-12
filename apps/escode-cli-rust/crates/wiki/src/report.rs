//! 导入/同步报告（spec §6「Import Report」、§9）：逐文件状态、告警、错误阶段与条目变化计数。
use serde::Serialize;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Item {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// imported | updated | unchanged | moved | missing | failed | degraded | skipped
    pub status: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created: usize,
    pub updated: usize,
    pub staled: usize,
    pub retired: usize,
    pub proposals: usize,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Report {
    pub scope: String,
    pub items: Vec<Item>,
}

impl Report {
    pub fn count(&self, status: &str) -> usize {
        self.items.iter().filter(|i| i.status == status).count()
    }

    pub fn ok(&self) -> bool {
        self.items.iter().all(|i| i.status != "failed")
    }

    pub fn summary(&self) -> String {
        let created: usize = self.items.iter().map(|i| i.created).sum();
        let updated: usize = self.items.iter().map(|i| i.updated).sum();
        let staled: usize = self.items.iter().map(|i| i.staled).sum();
        let proposals: usize = self.items.iter().map(|i| i.proposals).sum();
        let mut parts = vec![format!("{} file(s)", self.items.len())];
        for status in [
            "imported",
            "updated",
            "unchanged",
            "moved",
            "missing",
            "degraded",
            "skipped",
            "failed",
        ] {
            let n = self.count(status);
            if n > 0 {
                parts.push(format!("{n} {status}"));
            }
        }
        format!(
            "{} wiki: {}; entries created {created}, updated {updated}, marked stale {staled}, update proposals {proposals}",
            self.scope,
            parts.join(", ")
        )
    }
}
