//! SVN 工作副本快照（docs/svn/architecture.md「Agent 集成」）。
//!
//! 判定规则与 Host 的 `workspace-vcs` 及 TS `detectWorkspaceVcsKind` 一致：最近的 `.git` / `.svn`
//! 中更近者优先，同层 Git 优先。是 SVN 时由调用方跳过 Git 快照。命令复用 `context_git::command`
//! 的有界执行（3 秒超时、1 MiB 输出上限、可取消）；svn log 访问服务器，超时即省略日志。
use crate::domain::prompt::SvnSnapshot;
use std::path::Path;
use tokio_util::sync::CancellationToken;

const MAX_STATUS_UTF16_UNITS: usize = 2000;
const MAX_RECENT_LOGS: usize = 5;

/// 只做目录探测，不启动 svn / git。
pub(super) async fn is_svn_workspace(cwd: &Path, cancel: &CancellationToken) -> bool {
    let cwd = crate::realpath(cwd)
        .await
        .unwrap_or_else(|_| cwd.to_path_buf());
    let chain: Vec<&Path> = cwd.ancestors().collect();
    let (mut git_depth, mut svn_depth) = (None, None);
    for (index, dir) in chain.iter().enumerate() {
        if cancel.is_cancelled() {
            return false;
        }
        let depth = chain.len() - 1 - index;
        if git_depth.is_none() && tokio::fs::symlink_metadata(dir.join(".git")).await.is_ok() {
            git_depth = Some(depth);
        }
        if svn_depth.is_none()
            && tokio::fs::metadata(dir.join(".svn"))
                .await
                .is_ok_and(|meta| meta.is_dir())
        {
            svn_depth = Some(depth);
        }
        if git_depth.is_some() && svn_depth.is_some() {
            break;
        }
    }
    match (git_depth, svn_depth) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(git), Some(svn)) => svn > git,
    }
}

fn binary() -> String {
    escode_cli_core_api::env::var_os("ESCODE_SVN_BINARY")
        .map(|value| value.to_string_lossy().trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "svn".into())
}

async fn svn(cwd: &Path, args: &[&str], cancel: &CancellationToken) -> Option<String> {
    let mut full = vec!["--non-interactive"];
    full.extend_from_slice(args);
    super::context_git::command(cwd, &binary(), &full, cancel).await
}

fn xml_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))? + start;
    Some(xml[start..end].trim())
}

fn decode_xml(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn percent_decode(value: &str) -> String {
    let hex = |byte: u8| (byte as char).to_digit(16).map(|digit| digit as u8);
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push(high << 4 | low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 与 shared `deriveSvnBranchRef` 同规则：只认标准布局第一层目录名。
fn branch_label(relative: &str) -> String {
    let relative = relative.trim_matches('/');
    if relative == "trunk" || relative.starts_with("trunk/") {
        return "trunk".into();
    }
    for (prefix, tag) in [("branches/", false), ("tags/", true)] {
        if let Some(rest) = relative.strip_prefix(prefix) {
            let name = rest.split('/').next().unwrap_or_default();
            if !name.is_empty() {
                return if tag {
                    format!("tag: {name}")
                } else {
                    name.into()
                };
            }
        }
    }
    if relative.is_empty() {
        "/".into()
    } else {
        relative.into()
    }
}

fn recent_logs(xml: &str) -> String {
    let mut lines = vec![];
    let mut rest = xml;
    while let Some(start) = rest.find("<logentry") {
        let entry_rest = &rest[start..];
        let Some(end) = entry_rest.find("</logentry>") else {
            break;
        };
        let entry = &entry_rest[..end];
        let revision = entry
            .split("revision=\"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .unwrap_or_default();
        let author = xml_text(entry, "author")
            .map(decode_xml)
            .unwrap_or_default();
        let title = xml_text(entry, "msg")
            .map(decode_xml)
            .and_then(|msg| {
                msg.lines()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| line.trim().to_owned())
            })
            .unwrap_or_default();
        lines.push(if author.is_empty() {
            format!("r{revision} {title}")
        } else {
            format!("r{revision} {author}: {title}")
        });
        if lines.len() == MAX_RECENT_LOGS {
            break;
        }
        rest = &entry_rest[end..];
    }
    lines.join("\n")
}

fn truncate_status(status: &str) -> String {
    let status = status.trim_end().replace("\r\n", "\n");
    if status.encode_utf16().count() <= MAX_STATUS_UTF16_UNITS {
        return status;
    }
    let units: Vec<_> = status.encode_utf16().take(MAX_STATUS_UTF16_UNITS).collect();
    format!(
        "{}\n... (truncated; run \"svn status\" for the full list)",
        String::from_utf16_lossy(&units)
    )
}

pub(super) async fn snapshot(cwd: &Path, cancel: &CancellationToken) -> SvnSnapshot {
    let (info, status) = tokio::join!(
        svn(cwd, &["info", "--xml"], cancel),
        svn(cwd, &["status"], cancel),
    );
    let info = info.unwrap_or_default();
    let url = xml_text(&info, "url").map(decode_xml).unwrap_or_default();
    let root = xml_text(&info, "root").map(decode_xml).unwrap_or_default();
    let revision = info
        .find("<entry")
        .and_then(|start| info[start..].split("revision=\"").nth(1))
        .and_then(|tail| tail.split('"').next())
        .and_then(|value| value.parse().ok());
    let branch = (!root.is_empty() && url.starts_with(&root))
        .then(|| branch_label(&percent_decode(&url[root.len()..])))
        .unwrap_or_default();
    let logs = if url.is_empty() {
        String::new()
    } else {
        let limit = MAX_RECENT_LOGS.to_string();
        svn(cwd, &["log", "--xml", "-l", &limit, &url], cancel)
            .await
            .map(|xml| recent_logs(&xml))
            .unwrap_or_default()
    };
    SvnSnapshot {
        url,
        revision,
        branch,
        status: truncate_status(&status.unwrap_or_default()),
        recent_logs: logs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_label_follows_standard_layout() {
        assert_eq!(branch_label("/trunk/src"), "trunk");
        assert_eq!(branch_label("branches/feat/x"), "feat");
        assert_eq!(branch_label("tags/v1"), "tag: v1");
        assert_eq!(branch_label("project/main"), "project/main");
        assert_eq!(branch_label(""), "/");
        assert_eq!(percent_decode("branches/%E7%89%B9"), "branches/特");
    }

    #[test]
    fn recent_logs_take_first_message_line() {
        let xml = "<log><logentry revision=\"3\"><author>bob</author><msg>\n多行\nsecond</msg></logentry><logentry revision=\"2\"><msg>x &amp; y</msg></logentry></log>";
        assert_eq!(recent_logs(xml), "r3 bob: 多行\nr2 x & y");
    }

    #[tokio::test]
    async fn detection_matches_host_rule() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(root.path().join(".svn")).expect("svn");
        let nested = root.path().join("sub").join("inner");
        std::fs::create_dir_all(&nested).expect("nested");
        let cancel = CancellationToken::new();
        assert!(is_svn_workspace(&nested, &cancel).await);
        std::fs::write(root.path().join("sub").join(".git"), "gitdir: x\n").expect("git");
        assert!(!is_svn_workspace(&nested, &cancel).await);
        std::fs::create_dir(root.path().join(".git")).expect("git dir");
        assert!(!is_svn_workspace(root.path(), &cancel).await);
    }
}
