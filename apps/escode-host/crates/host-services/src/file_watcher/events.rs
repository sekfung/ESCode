//! notify 事件 → TS `fs.watch` 回调口径的变更路径（docs/specs/rust-u2-file-watcher.md §3）。

use notify::{Event, EventKind};

/// 一次底层变更对防抖窗口的贡献。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// 能确认的绝对路径（TS `resolveFileWatchChangedPath` 的结果）。
    Path(String),
    /// 底层没给出可用文件名（TS `fileName === null` 或空串）。
    Unknown,
    /// 底层 watcher 报错（TS `fsWatcher.on("error")`）。
    Error(String),
}

fn base_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
}

/// 把底层回报的「解析软链后的真实路径」换回调用方给的 root 前缀（TS `fs.watch` 只给相对文件名，
/// `changedPath = resolve(dirPath, fileName)`，前缀恒为调用方路径）。
///
/// 修复：macOS FSEvents 把 `/var/...` 报成 `/private/var/...`，原样转发时 `changedPath` 与 `dirPath`
/// 前缀不一致，根目录自身事件也认不出来（macos-latest test 作业暴露）。`canonical` 为 root 的
/// 真实路径；与 root 相同或取不到时不改写。
pub fn rebase_event(mut event: Event, root: &str, canonical: Option<&str>) -> Event {
    let Some(canonical) = canonical.map(|value| value.trim_end_matches(['/', '\\'])) else {
        return event;
    };
    let root = root.trim_end_matches(['/', '\\']);
    if canonical == root || canonical.is_empty() {
        return event;
    }
    for path in &mut event.paths {
        let text = path.to_string_lossy().into_owned();
        if let Some(rest) = text.strip_prefix(canonical)
            && (rest.is_empty() || rest.starts_with(['/', '\\']))
        {
            *path = std::path::PathBuf::from(format!("{root}{rest}"));
        }
    }
    event
}

/// Node/inotify 实测：被监视目录自身变化时，非递归报 `fileName=<目录 basename>`
/// （`resolve(dir, basename)`，一个不存在的同名子路径），递归报空 `fileName`（未知路径）。
pub fn map_event(root: &str, recursive: bool, event: &Event) -> Vec<Change> {
    // Access（open / close-write）不在 Node 的 inotify 掩码内。
    if matches!(event.kind, EventKind::Access(_)) {
        return Vec::new();
    }
    if event.paths.is_empty() {
        return vec![Change::Unknown];
    }
    let root_trimmed = root.trim_end_matches(['/', '\\']);
    event
        .paths
        .iter()
        .map(|path| {
            let path = path.to_string_lossy();
            if path.trim_end_matches(['/', '\\']) == root_trimmed {
                if recursive {
                    Change::Unknown
                } else {
                    // 修复：TS `resolve(dir, basename)` 用平台分隔符；写死 `/` 在 Windows 上与 TS 不一致
                    // （Windows 路径扫描发现）。
                    Change::Path(format!(
                        "{root_trimmed}{}{}",
                        std::path::MAIN_SEPARATOR,
                        base_name(root_trimmed)
                    ))
                }
            } else {
                Change::Path(path.into_owned())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind};

    use super::*;

    #[test]
    fn rebase_maps_canonical_paths_back_to_the_caller_root() {
        let created = event(
            EventKind::Create(CreateKind::File),
            &["/private/var/t/a.txt", "/private/var/t", "/elsewhere/b"],
        );
        let rebased = rebase_event(created, "/var/t", Some("/private/var/t"));
        let paths: Vec<String> = rebased
            .paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(paths, ["/var/t/a.txt", "/var/t", "/elsewhere/b"]);
        // 前缀须在路径边界上：/private/var/tx 不属于 /private/var/t。
        let sibling = event(EventKind::Create(CreateKind::File), &["/private/var/tx/a"]);
        let kept = rebase_event(sibling, "/var/t", Some("/private/var/t"));
        assert_eq!(kept.paths[0].to_string_lossy(), "/private/var/tx/a");
    }

    fn event(kind: EventKind, paths: &[&str]) -> Event {
        let mut event = Event::new(kind);
        for path in paths {
            event = event.add_path(std::path::PathBuf::from(path));
        }
        event
    }

    #[test]
    fn access_events_are_ignored() {
        let close = event(
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            &["/w/a"],
        );
        assert!(map_event("/w", false, &close).is_empty());
    }

    #[test]
    fn child_paths_pass_through_and_pathless_events_are_unknown() {
        let created = event(EventKind::Create(CreateKind::File), &["/w/a.txt"]);
        assert_eq!(
            map_event("/w", false, &created),
            vec![Change::Path("/w/a.txt".into())]
        );
        let modified = event(EventKind::Modify(ModifyKind::Any), &[]);
        assert_eq!(map_event("/w", true, &modified), vec![Change::Unknown]);
    }

    #[test]
    fn root_itself_maps_like_node_inotify() {
        let removed = event(
            EventKind::Remove(notify::event::RemoveKind::Folder),
            &["/w/dir/"],
        );
        assert_eq!(
            map_event("/w/dir", false, &removed),
            // 根自身事件用平台分隔符（TS resolve 同），Windows 上为 `/w/dir\\dir`。
            vec![Change::Path(format!(
                "/w/dir{}dir",
                std::path::MAIN_SEPARATOR
            ))]
        );
        assert_eq!(map_event("/w/dir", true, &removed), vec![Change::Unknown]);
    }
}
