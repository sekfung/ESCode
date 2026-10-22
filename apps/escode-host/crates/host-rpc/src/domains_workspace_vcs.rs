//! `workspace-vcs` channel（TS `ServiceChannels.WorkspaceVcs`；docs/svn/architecture.md）。
//!
//! workspace VCS 类型（git / svn / none）的唯一判定入口：只做目录探测与覆盖值读写，
//! 规则与 TS `createWorkspaceVcsService` 一致——最近的 `.git` / `.svn` 中更深者优先，
//! 同层 Git 优先；覆盖值按身份 key（`workspaceIdentity?.trim() || workspacePath`）
//! 存在设置 `workspaceVcsOverrides` 里。`svn` channel 在 Rust Host 第一期不实现，
//! 由渲染器把调用失败呈现为“当前 Host 暂不支持 SVN”。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::channel::Channel;
use crate::domains_task_support::first_object;
use crate::session::handler;
use crate::wire::Wire;

const OVERRIDES_KEY: &str = "workspaceVcsOverrides";
const OVERRIDE_VALUES: [&str; 4] = ["auto", "none", "git", "svn"];

#[derive(Debug, Default, PartialEq, Eq)]
struct Markers {
    git_root: Option<PathBuf>,
    git_depth: Option<usize>,
    svn_root: Option<PathBuf>,
    svn_depth: Option<usize>,
}

async fn find_markers(workspace: &Path) -> Markers {
    let chain: Vec<&Path> = workspace.ancestors().collect();
    let mut markers = Markers::default();
    for (index, dir) in chain.iter().enumerate() {
        let depth = chain.len() - 1 - index;
        if markers.git_root.is_none() && tokio::fs::metadata(dir.join(".git")).await.is_ok() {
            markers.git_root = Some(dir.to_path_buf());
            markers.git_depth = Some(depth);
        }
        if markers.svn_root.is_none()
            && tokio::fs::metadata(dir.join(".svn"))
                .await
                .is_ok_and(|meta| meta.is_dir())
        {
            markers.svn_root = Some(dir.to_path_buf());
            markers.svn_depth = Some(depth);
        }
        if markers.git_root.is_some() && markers.svn_root.is_some() {
            break;
        }
    }
    markers
}

fn detected_kind(markers: &Markers) -> &'static str {
    match (markers.git_depth, markers.svn_depth) {
        (None, None) => "none",
        (Some(_), None) => "git",
        (None, Some(_)) => "svn",
        (Some(git), Some(svn)) => {
            if svn > git {
                "svn"
            } else {
                "git"
            }
        }
    }
}

fn root_for<'a>(kind: &str, markers: &'a Markers) -> Option<&'a Path> {
    match kind {
        "git" => markers.git_root.as_deref(),
        "svn" => markers.svn_root.as_deref(),
        _ => None,
    }
}

fn identity_key(params: &Value) -> String {
    let identity = params
        .get("workspaceIdentity")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if identity.is_empty() {
        text(params, "workspacePath")
    } else {
        identity.to_owned()
    }
}

fn text(params: &Value, key: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

async fn describe(workspace_path: &str, override_value: &str) -> Value {
    let markers = find_markers(Path::new(workspace_path)).await;
    let (kind, source) = if override_value == "auto" {
        (detected_kind(&markers), "detected")
    } else {
        (override_value, "override")
    };
    let mut info = json!({ "kind": kind, "source": source, "override": override_value });
    if let Some(root) = root_for(kind, &markers) {
        info["root"] = Value::String(root.to_string_lossy().into_owned());
    }
    info
}

fn read_override(settings: &Value, key: &str) -> String {
    settings
        .get(OVERRIDES_KEY)
        .and_then(|overrides| overrides.get(key))
        .and_then(Value::as_str)
        .filter(|value| OVERRIDE_VALUES.contains(value))
        .unwrap_or("auto")
        .to_owned()
}

pub(crate) fn workspace_vcs_channel(home: PathBuf) -> Channel {
    let read_home = home.clone();
    let write_home = home;
    Channel::new(
        "workspace-vcs",
        vec![
            (
                "getWorkspaceVcsInfo",
                handler(move |arg: Wire| {
                    let home = read_home.clone();
                    Box::pin(async move {
                        let params = first_object(arg);
                        let settings = escode_host_services::settings::read(&home)
                            .await
                            .unwrap_or(Value::Null);
                        let override_value = read_override(&settings, &identity_key(&params));
                        Ok(Wire::Object(
                            describe(&text(&params, "workspacePath"), &override_value).await,
                        ))
                    })
                }),
            ),
            (
                "setWorkspaceVcsOverride",
                handler(move |arg: Wire| {
                    let home = write_home.clone();
                    Box::pin(async move {
                        let params = first_object(arg);
                        let override_value = text(&params, "override");
                        if !OVERRIDE_VALUES.contains(&override_value.as_str()) {
                            return Err((
                                "Error".to_owned(),
                                format!("invalid workspace VCS override: {override_value}"),
                            ));
                        }
                        let settings = escode_host_services::settings::read(&home)
                            .await
                            .map_err(|error| ("Error".to_owned(), format!("{error:#}")))?;
                        let mut overrides = settings
                            .get(OVERRIDES_KEY)
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_else(Map::new);
                        let key = identity_key(&params);
                        if override_value == "auto" {
                            overrides.remove(&key);
                        } else {
                            overrides.insert(key, Value::String(override_value.clone()));
                        }
                        escode_host_services::settings::set_key(
                            &home,
                            OVERRIDES_KEY,
                            Some(Value::Object(overrides)),
                        )
                        .await
                        .map_err(|error| ("Error".to_owned(), format!("{error:#}")))?;
                        Ok(Wire::Object(
                            describe(&text(&params, "workspacePath"), &override_value).await,
                        ))
                    })
                }),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nearest_marker_wins_and_git_wins_ties() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(root.path().join(".svn")).expect("svn dir");
        let nested = root.path().join("sub").join("inner");
        std::fs::create_dir_all(&nested).expect("nested");

        let info = describe(nested.to_str().expect("utf8"), "auto").await;
        assert_eq!(info["kind"], "svn");
        assert_eq!(info["root"], root.path().to_string_lossy().as_ref());

        std::fs::write(root.path().join("sub").join(".git"), "gitdir: x\n").expect("git file");
        assert_eq!(describe(nested.to_str().expect("utf8"), "auto").await["kind"], "git");

        std::fs::create_dir(root.path().join(".git")).expect("git dir");
        assert_eq!(
            describe(root.path().to_str().expect("utf8"), "auto").await["kind"],
            "git"
        );
    }

    #[tokio::test]
    async fn override_does_not_fall_back() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(root.path().join(".git")).expect("git dir");
        let info = describe(root.path().to_str().expect("utf8"), "svn").await;
        assert_eq!(info, json!({ "kind": "svn", "source": "override", "override": "svn" }));
    }

    #[test]
    fn identity_key_prefers_trimmed_identity() {
        assert_eq!(
            identity_key(&json!({ "workspacePath": "/w", "workspaceIdentity": " ssh://h/r " })),
            "ssh://h/r"
        );
        assert_eq!(identity_key(&json!({ "workspacePath": "/w" })), "/w");
        assert_eq!(read_override(&json!({ OVERRIDES_KEY: { "/w": "bogus" } }), "/w"), "auto");
    }
}
