//! `system.listIntegratedTerminalShells` / `probeIntranet`（TS `systemService.ts`、`integratedTerminalShells.ts`）。
//! docs/specs/rust-u2-user-config-services.md §5。

use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use tokio::net::TcpStream;

use crate::git::paths::js_trim;

const DEFAULT_TIMEOUT_MS: u64 = 800;
const DEFAULT_ATTEMPTS: u64 = 2;
const MAX_ATTEMPTS: u64 = 3;
const DEFAULT_PORT: u64 = 22;

// ───────────────────────── 终端 shell 发现（仅 Windows 有选项） ─────────────────────────

fn env_value(lookup: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    lookup(key)
}

/// `win32.normalize` 的子集：统一分隔符、折叠重复分隔符、解析 `.`/`..`。
fn win32_normalize(path: &str) -> String {
    let unified = path.replace('/', "\\");
    let (prefix, rest) = match unified.split_once(':') {
        Some((drive, rest)) if drive.len() == 1 => (format!("{drive}:\\"), rest.to_owned()),
        _ if unified.starts_with('\\') => ("\\".to_owned(), unified.clone()),
        _ => (String::new(), unified.clone()),
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in rest.split('\\') {
        match segment {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if prefix.is_empty() {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    format!("{prefix}{}", parts.join("\\"))
}

fn win32_dirname(path: &str) -> String {
    let normalized = path.replace('/', "\\");
    match normalized.rfind('\\') {
        Some(0) => "\\".to_owned(),
        Some(index) => normalized[..index].to_owned(),
        None => ".".to_owned(),
    }
}

fn executable_extensions(lookup: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let raw = env_value(lookup, "PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned());
    let normalized: Vec<String> = raw
        .split(';')
        .map(|extension| js_trim(extension).to_lowercase())
        .filter(|extension| !extension.is_empty())
        .collect();
    if normalized.iter().any(|extension| extension == ".exe") {
        normalized
    } else {
        std::iter::once(".exe".to_owned())
            .chain(normalized)
            .collect()
    }
}

fn git_candidates(lookup: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let Some(path) = env_value(lookup, "PATH").filter(|value| !value.is_empty()) else {
        return vec!["git".to_owned()];
    };
    let extensions = executable_extensions(lookup);
    path.split(';')
        .filter(|entry| !js_trim(entry).is_empty())
        .flat_map(|entry| {
            extensions
                .iter()
                .map(move |extension| win32_normalize(&format!("{entry}\\git{extension}")))
        })
        .collect()
}

/// TS `listIntegratedTerminalShellOptions`：非 Windows 恒为空；Windows 给 CMD 与（若找到）Git Bash。
pub fn list_integrated_terminal_shells(
    is_windows: bool,
    lookup: &dyn Fn(&str) -> Option<String>,
    is_executable: &dyn Fn(&str) -> bool,
) -> Vec<Value> {
    if !is_windows {
        return Vec::new();
    }
    let cmd_path = env_value(lookup, "ComSpec")
        .map(|value| js_trim(&value).to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "cmd.exe".to_owned());
    let mut options = vec![json!({
        "dialect": "cmd", "id": format!("cmd:{cmd_path}"), "label": "CMD", "path": cmd_path, "source": "system",
    })];
    if let Some((path, source)) = resolve_git_bash(lookup, is_executable) {
        options.push(json!({
            "dialect": "git-bash", "id": format!("git-bash:{path}"), "label": "Git Bash",
            "path": path, "source": source,
        }));
    }
    options
}

fn resolve_git_bash(
    lookup: &dyn Fn(&str) -> Option<String>,
    is_executable: &dyn Fn(&str) -> bool,
) -> Option<(String, &'static str)> {
    for candidate in [
        "C:\\Program Files\\Git\\bin\\bash.exe",
        "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
    ] {
        if is_executable(candidate) {
            return Some((candidate.to_owned(), "system"));
        }
    }
    let git = git_candidates(lookup)
        .into_iter()
        .find(|c| is_executable(c))?;
    let dir = win32_dirname(&git);
    [
        win32_normalize(&format!("{dir}\\..\\bin\\bash.exe")),
        win32_normalize(&format!("{dir}\\..\\..\\bin\\bash.exe")),
    ]
    .into_iter()
    .find(|candidate| is_executable(candidate))
    .map(|path| (path, "path"))
}

// ───────────────────────── 内网探测 ─────────────────────────

enum Target {
    Tcp {
        id: String,
        host: String,
        port: u64,
        timeout_ms: u64,
    },
    Service {
        id: String,
        url: String,
        marker: Option<String>,
        token: Option<String>,
        timeout_ms: u64,
    },
}

fn timeout_of(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .map_or(DEFAULT_TIMEOUT_MS, |n| {
            n.floor().clamp(100.0, 10_000.0) as u64
        })
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|text| js_trim(text).to_owned())
        .filter(|text| !text.is_empty())
}

fn normalize_target(target: &Value) -> Option<Target> {
    if target["kind"] == "service" {
        let text = js_trim(target["url"].as_str().unwrap_or_default());
        let parsed = url::Url::parse(text)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))?;
        return Some(Target::Service {
            id: non_empty(target.get("id")).unwrap_or_else(|| parsed.to_string()),
            url: parsed.to_string(),
            marker: non_empty(target.get("expectedMarker")),
            token: non_empty(target.get("token")),
            timeout_ms: timeout_of(target.get("timeoutMs")),
        });
    }
    let host = js_trim(target["host"].as_str().unwrap_or_default());
    if host.is_empty() {
        return None;
    }
    let port = target["port"]
        .as_f64()
        .filter(|p| p.fract() == 0.0 && (1.0..=65_535.0).contains(p))
        .map_or(DEFAULT_PORT, |p| p as u64);
    Some(Target::Tcp {
        id: non_empty(target.get("id")).unwrap_or_else(|| format!("{host}:{port}")),
        host: host.to_owned(),
        port,
        timeout_ms: timeout_of(target.get("timeoutMs")),
    })
}

async fn tcp_probe(host: &str, port: u64, timeout_ms: u64) -> Result<u64, String> {
    let started = Instant::now();
    let connect = TcpStream::connect((host, u16::try_from(port).unwrap_or(22)));
    match tokio::time::timeout(Duration::from_millis(timeout_ms), connect).await {
        Ok(Ok(_)) => Ok(started.elapsed().as_millis() as u64),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(format!("timeout({timeout_ms}ms)")),
    }
}

async fn service_probe(
    url: &str,
    token: Option<&str>,
    timeout_ms: u64,
) -> Result<(u64, Option<String>), String> {
    let started = Instant::now();
    let client = escode_cli_net::client(
        escode_cli_net::ClientOptions::new(escode_cli_net::Purpose::WebFetch, url)
            .timeout(Duration::from_millis(timeout_ms)),
    )
    .await
    .map_err(|error| format!("{error:#}"))?;
    escode_cli_net::capture::record("GET", url);
    let mut request = client.get(url);
    if let Some(token) = token {
        request = request.header("x-zcode-intranet-token", token);
    }
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            format!("timeout({timeout_ms}ms)")
        } else {
            error.to_string()
        }
    })?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let body: Value = serde_json::from_str(&response.text().await.map_err(|e| e.to_string())?)
        .map_err(|_| "invalid service response".to_owned())?;
    let Some(object) = body.as_object() else {
        return Err("invalid service response".to_owned());
    };
    let Some(ok) = object.get("ok").and_then(Value::as_bool) else {
        return Err("invalid service response: missing ok".to_owned());
    };
    let marker = match object.get("marker") {
        None | Some(Value::Null) => None,
        Some(Value::String(marker)) => Some(marker.clone()),
        Some(_) => return Err("invalid service response: marker must be string".to_owned()),
    };
    if !ok {
        return Err("service returned ok=false".to_owned());
    }
    Ok((started.elapsed().as_millis() as u64, marker))
}

async fn run_target(target: &Target, attempts: u64) -> Value {
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        match target {
            Target::Tcp {
                id,
                host,
                port,
                timeout_ms,
            } => match tcp_probe(host, *port, *timeout_ms).await {
                Ok(latency) => {
                    return json!({"targetId": id, "kind": "tcp", "host": host, "port": port,
                        "reachable": true, "attemptCount": attempt, "latencyMs": latency});
                }
                Err(error) => last_error = error,
            },
            Target::Service {
                id,
                url,
                marker,
                token,
                timeout_ms,
            } => match service_probe(url, token.as_deref(), *timeout_ms).await {
                Ok((latency, actual)) => {
                    if let Some(expected) = marker
                        && actual.as_deref() != Some(expected.as_str())
                    {
                        last_error = format!(
                            "marker mismatch(expected={expected}, actual={})",
                            actual.as_deref().unwrap_or("<empty>")
                        );
                        continue;
                    }
                    let mut result = json!({"targetId": id, "kind": "service", "url": url,
                            "reachable": true, "attemptCount": attempt, "latencyMs": latency});
                    if let Some(actual) = actual {
                        result["marker"] = Value::from(actual);
                    }
                    return result;
                }
                Err(error) => last_error = error,
            },
        }
    }
    let error = if last_error.is_empty() {
        "probe failed".to_owned()
    } else {
        last_error
    };
    match target {
        Target::Tcp { id, host, port, .. } => {
            json!({"targetId": id, "kind": "tcp", "host": host, "port": port,
            "reachable": false, "attemptCount": attempts, "latencyMs": null, "error": error})
        }
        Target::Service { id, url, .. } => json!({"targetId": id, "kind": "service", "url": url,
            "reachable": false, "attemptCount": attempts, "latencyMs": null, "error": error}),
    }
}

/// TS `probeIntranet`。
pub async fn probe_intranet(request: &Value) -> Result<Value, String> {
    // TS 直接 `request.targets.map(...)`：缺 targets 是 TypeError。
    let Some(list) = request["targets"].as_array() else {
        return Err("Cannot read properties of undefined (reading 'map')".to_owned());
    };
    let targets: Vec<Target> = list.iter().filter_map(normalize_target).collect();
    let attempts = request["attempts"]
        .as_f64()
        .filter(|n| n.is_finite())
        .map_or(DEFAULT_ATTEMPTS, |n| {
            n.floor().clamp(1.0, MAX_ATTEMPTS as f64) as u64
        });
    let total = targets.len() as u64;
    let required = if total == 0 {
        1
    } else {
        request["requiredSuccessCount"]
            .as_f64()
            .filter(|n| n.is_finite())
            .map_or(1, |n| n.floor().clamp(1.0, total as f64) as u64)
    };
    // 与 `Promise.all` 同：并发等待、按输入顺序返回。
    let results =
        futures_util::future::join_all(targets.iter().map(|target| run_target(target, attempts)))
            .await;
    let reached = results.iter().filter(|r| r["reachable"] == true).count() as u64;
    let strategy = if targets.iter().all(|t| matches!(t, Target::Tcp { .. })) {
        "tcp-connect"
    } else if targets.iter().all(|t| matches!(t, Target::Service { .. })) {
        "service-http"
    } else {
        "mixed"
    };
    let mut result = Map::new();
    result.insert(
        "isIntranet".into(),
        Value::Bool(total > 0 && reached >= required),
    );
    result.insert("reachedTargetCount".into(), json!(reached));
    result.insert("requiredSuccessCount".into(), json!(required));
    result.insert("totalTargets".into(), json!(total));
    result.insert(
        "checkedAt".into(),
        json!(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64)
        ),
    );
    result.insert("strategy".into(), json!(strategy));
    result.insert("results".into(), Value::Array(results));
    Ok(Value::Object(result))
}

#[cfg(test)]
#[path = "system_probe_tests.rs"]
mod tests;
