//! 资源管理器 host 进程采样（`resource-usage-snapshot-request` 的 Rust 实现）。
//!
//! Node Host 用 `processResourceSampler` 采样本进程树；Rust Host 的实例默认 embedded，
//! 没有 per-workspace CLI 子进程，因此行集合是 Host 自身 + 其子进程树（bash/MCP/terminal）。
//! 输出形状对齐共享 `resource-usage-snapshot-result`。

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct ProcessRow {
    pid: u32,
    ppid: u32,
    name: String,
    rss_bytes: u64,
    cpu_percent: f64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// 一次采样结果（`HostEvent.event`）。
pub fn snapshot_event(request_id: &str) -> Value {
    json!({
        "type": "resource-usage-snapshot-result",
        "requestId": request_id,
        "sampledAt": now_ms(),
        "processes": rows(),
    })
}

fn rows() -> Vec<Value> {
    let table = read_process_table();
    let root = std::process::id();
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for row in &table {
        children.entry(row.ppid).or_default().push(row.pid);
    }
    let mut selected = vec![root];
    let mut index = 0;
    while index < selected.len() {
        let pid = selected[index];
        index += 1;
        if let Some(next) = children.get(&pid) {
            selected.extend(next.iter().copied());
        }
    }
    let by_pid: BTreeMap<u32, &ProcessRow> = table.iter().map(|row| (row.pid, row)).collect();
    selected
        .into_iter()
        .filter_map(|pid| by_pid.get(&pid).map(|row| (pid, *row)))
        .take(10_000)
        .map(|(pid, row)| {
            json!({
                "pid": pid,
                "name": row.name,
                "category": "base",
                "groupKey": "host",
                "groupLabel": "ESCode Host",
                "cpuPercent": round_one(row.cpu_percent),
                "memoryBytes": row.rss_bytes,
            })
        })
        .collect()
}

fn round_one(value: f64) -> f64 {
    (value.clamp(0.0, 100.0) * 10.0).round() / 10.0
}

#[cfg(target_os = "linux")]
fn read_process_table() -> Vec<ProcessRow> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut table = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some(open) = stat.find('(') else { continue };
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        let comm = stat[open + 1..close].to_owned();
        let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        // rest[0] = state, rest[1] = ppid, ...; rss 是第 22 个字段（stat 第 24 个）。
        let ppid = rest
            .get(1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let rss_kb = rest
            .get(21)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let cpu_percent = cpu_percent(
            rest.get(11).copied(),
            rest.get(12).copied(),
            rest.get(13).copied(),
        );
        table.push(ProcessRow {
            pid,
            ppid,
            name: comm,
            rss_bytes: rss_kb.saturating_mul(1024),
            cpu_percent,
        });
    }
    table
}

/// `/proc/<pid>/stat` 的 utime/stime/starttime 无法给出瞬时百分比（没有上一次基线），
/// 用 0 表示“本采样窗口未计算”，与 Node 首帧语义一致（首帧 CPU 为 0）。
#[cfg(target_os = "linux")]
fn cpu_percent(_utime: Option<&str>, _stime: Option<&str>, _starttime: Option<&str>) -> f64 {
    0.0
}

#[cfg(target_os = "macos")]
fn read_process_table() -> Vec<ProcessRow> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss=,comm="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let rss_kb: u64 = parts.next()?.parse().ok()?;
            let name = parts.collect::<Vec<_>>().join(" ");
            Some(ProcessRow {
                pid,
                ppid,
                name,
                rss_bytes: rss_kb.saturating_mul(1024),
                cpu_percent: 0.0,
            })
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn read_process_table() -> Vec<ProcessRow> {
    let script = "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $($_.WorkingSetSize) $($_.Name)\" }";
    let Ok(output) = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let rss_bytes = parts.next()?.parse().ok()?;
            let name = parts.collect::<Vec<_>>().join(" ");
            Some(ProcessRow {
                pid,
                ppid,
                name,
                rss_bytes,
                cpu_percent: 0.0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_contains_the_host_process() {
        let event = snapshot_event("req-1");
        assert_eq!(event["type"], "resource-usage-snapshot-result");
        assert_eq!(event["requestId"], "req-1");
        let processes = event["processes"].as_array().expect("processes");
        assert!(processes.iter().any(|row| row["pid"] == std::process::id()));
        assert!(processes.iter().all(|row| row["category"] == "base"));
    }
}
