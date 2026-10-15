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
                    Change::Path(format!("{root_trimmed}/{}", base_name(root_trimmed)))
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
            vec![Change::Path("/w/dir/dir".into())]
        );
        assert_eq!(map_event("/w/dir", true, &removed), vec![Change::Unknown]);
    }
}
