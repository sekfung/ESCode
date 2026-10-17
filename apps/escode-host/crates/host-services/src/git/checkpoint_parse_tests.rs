//! `git-checkpoint` 的三个 `-z` 输出解析（`rust-u2-git.md` §8.4）。
//! 每条用例都对应 TS `gitCheckpointHelpers.ts` 里一处具体的推进/跳过分支。
use super::*;

#[test]
fn name_status_maps_the_four_kinds_and_rename_consumes_three_records() {
    let entries = parse_name_status("M\0mod.txt\0A\0add.txt\0D\0del.txt\0R100\0old.txt\0new.txt\0");
    let kinds: Vec<(&str, &str, Option<&str>)> = entries
        .iter()
        .map(|entry| {
            (
                entry.kind,
                entry.path.as_str(),
                entry.original_path.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("modified", "mod.txt", None),
            ("added", "add.txt", None),
            ("deleted", "del.txt", None),
            ("renamed", "new.txt", Some("old.txt")),
        ]
    );
    // 未知状态字母归 modified；copy（C）与 rename 同族。
    let entries = parse_name_status("X\0weird.txt\0C90\0src.txt\0dst.txt\0");
    assert_eq!(entries[0].kind, "modified");
    assert_eq!(entries[1].kind, "renamed");
}

/// 畸形记录整条跳过、不补位：rename 缺新路径时不能把后面一条当成新路径。
#[test]
fn name_status_skips_malformed_records_without_shifting() {
    assert!(parse_name_status("R100\0old.txt\0").is_empty());
    // 缺 path 的单条记录也被跳过（TS `!path` 分支）。
    assert!(parse_name_status("M\0").is_empty());
    // 空输出。
    assert!(parse_name_status("").is_empty());
}

#[test]
fn numstat_handles_binary_renames_and_tabbed_paths() {
    let stats = parse_numstat("3\t1\ta.txt\0-\t-\tbin.dat\0");
    assert_eq!(
        stats,
        vec![("a.txt".to_owned(), 3, 1), ("bin.dat".to_owned(), 0, 0),]
    );

    // rename：path 字段为空，改从后续两条记录取；**新旧路径各写一条**（旧路径也拿到同一统计）。
    let stats = parse_numstat("2\t0\t\0old.txt\0new.txt\0");
    assert_eq!(
        stats,
        vec![("new.txt".to_owned(), 2, 0), ("old.txt".to_owned(), 2, 0),]
    );

    // 字段不足三条的记录跳过；路径本身含 tab 时由 slice(2).join 还原。
    assert!(parse_numstat("1\t2\0").is_empty());
    let stats = parse_numstat("1\t1\ta\tb.txt\0");
    assert_eq!(stats, vec![("a\tb.txt".to_owned(), 1, 1)]);

    // 非法计数归 0（TS `parseInt(v, 10) || 0`）。
    let stats = parse_numstat("abc\t5\tx.txt\0");
    assert_eq!(stats, vec![("x.txt".to_owned(), 0, 5)]);
}

#[test]
fn ls_tree_reads_mode_and_object_id_and_skips_bad_records() {
    let entries = parse_ls_tree("100644 blob abc123\tfile.txt\0");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "file.txt");
    assert_eq!(entries[0].1.mode, "100644");
    assert_eq!(entries[0].1.object_id, "abc123");

    // 目录模式与符号链接模式原样保留（冲突检测按 `120000` 判符号链接）。
    let entries = parse_ls_tree("120000 blob dead00\tlink\0");
    assert_eq!(entries[0].1.mode, "120000");

    // 缺 tab、header 段数不足：跳过。
    assert!(parse_ls_tree("100644 blob abc123 file.txt\0").is_empty());
    assert!(parse_ls_tree("100644\tfile.txt\0").is_empty());
    assert!(parse_ls_tree("").is_empty());
}
