//! 项目身份的纯规则（spec §4）：Git remote 规范化、`.git/config` 解析、gitfile 解析与 Project ID 派生。
//! 文件系统遍历（realpath、向上找 `.git`、读文件）在 escode-cli-wiki 适配器里。
use super::text::{sha256_hex, slug};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentityKind {
    GitRemote,
    GitLocal,
    WorkspaceIdentity,
    Path,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectIdentity {
    pub id: String,
    pub name: String,
    pub kind: IdentityKind,
    /// 身份键（如 `git-remote:github.com/org/repo`）；本地路径类键只用于哈希与诊断。
    pub key: String,
    /// 项目根（Git 工作树根或 workspace 路径）；用于相对来源路径与变更文件。
    pub root: String,
}

pub fn project_id(name: &str, key: &str) -> String {
    format!("{}-{}", slug(name, 40), &sha256_hex(key.as_bytes())[..16])
}

/// 规范化 Git remote：去协议、凭据、端口前缀写法差异、`.git` 后缀与尾斜杠，host 小写。
/// `git@github.com:Org/Repo.git` 与 `https://user@github.com/Org/Repo` 得到同一键 `github.com/Org/Repo`。
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = host.split(':').next().unwrap_or(host);
        (host.to_owned(), path.to_owned())
    } else if let Some((left, path)) = url.split_once(':')
        && !left.contains('/')
        && left.len() > 1
    {
        // scp 形式 user@host:path；单字母左侧是 Windows 盘符（本地路径 remote）。
        let host = left.rsplit_once('@').map_or(left, |(_, h)| h);
        (host.to_owned(), path.to_owned())
    } else {
        // 本地路径 remote：保留原样（规范分隔符）。
        return Some(format!(
            "local:{}",
            url.replace('\\', "/").trim_end_matches('/')
        ));
    };
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_lowercase(), path))
}

/// 从 `.git/config` 文本取 remote：优先 origin，否则按名字字典序第一个。
pub fn pick_remote(config: &str) -> Option<String> {
    let mut remotes: Vec<(String, String)> = vec![];
    let mut current: Option<String> = None;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            current = parse_remote_section(line);
            continue;
        }
        if let Some(name) = &current
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("url")
        {
            remotes.push((name.clone(), value.trim().trim_matches('"').to_owned()));
        }
    }
    remotes.sort();
    remotes
        .iter()
        .find(|(name, _)| name == "origin")
        .or_else(|| remotes.first())
        .map(|(_, url)| url.clone())
}

fn parse_remote_section(line: &str) -> Option<String> {
    let inner = line.trim_start_matches('[').trim_end_matches(']').trim();
    let rest = inner.strip_prefix("remote")?;
    let name = rest.trim().trim_matches('"');
    (!name.is_empty()).then(|| name.to_owned())
}

/// gitfile 内容 `gitdir: <path>` → 路径文本。
pub fn parse_gitfile(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
}

/// 路径类身份键：大小写不敏感文件系统上小写化，分隔符统一为 `/`。
pub fn path_key(path: &str, case_insensitive: bool) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let normalized = if normalized.is_empty() {
        "/"
    } else {
        normalized
    };
    if case_insensitive {
        normalized.to_lowercase()
    } else {
        normalized.to_owned()
    }
}

/// 显示名：remote 取最后一段，路径取最后一个目录名。
pub fn display_name(key_source: &str) -> String {
    key_source
        .trim_end_matches('/')
        .rsplit(['/', '\\', ':'])
        .find(|s| !s.is_empty())
        .unwrap_or("project")
        .to_owned()
}
