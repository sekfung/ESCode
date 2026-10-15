//! 解析器单测（期望值逐条对照 TS `gitCliHelpers.ts` / `gitCliRepo.ts` 的同名函数行为）。

use super::*;
use crate::git::command::GitOutput;

fn output(exit_code: Option<i32>, stdout: &str, stderr: &str) -> GitOutput {
    GitOutput {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
        timed_out: false,
        output_truncated: false,
        duration_ms: 0,
        timeout_ms: 15_000,
        timeout_elapsed_ms: None,
    }
}

#[test]
fn js_parse_int_follows_number_parse_int() {
    assert_eq!(js_parse_int("  42abc"), Some(42));
    assert_eq!(js_parse_int("-7"), Some(-7));
    assert_eq!(js_parse_int("+3"), Some(3));
    assert_eq!(js_parse_int("abc"), None);
    assert_eq!(js_parse_int(""), None);
    assert_eq!(js_parse_int("-"), None);
}

#[test]
fn porcelain_v2_parses_headers_and_all_record_kinds() {
    let stdout = [
        "# branch.oid 0123",
        "# branch.head main",
        "# branch.upstream origin/main",
        "# branch.ab +2 -3",
        "1 M. N... 100644 100644 100644 aaa bbb src/a file.ts",
        "1 .D N... 100644 100644 000000 aaa bbb gone.ts",
        "2 R. N... 100644 100644 100644 aaa bbb R100 new.ts",
        "old.ts",
        "u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.ts",
        "? notes/todo.md",
        "",
    ]
    .join("\0");
    let parsed = parse_status_porcelain(&stdout);
    assert_eq!(parsed.branch_name.as_deref(), Some("main"));
    assert_eq!(parsed.tracking_branch_name.as_deref(), Some("origin/main"));
    assert_eq!((parsed.ahead, parsed.behind), (2, 3));
    let summary: Vec<(&str, &str, Option<&str>, bool, bool)> = parsed
        .entries
        .iter()
        .map(|entry| {
            (
                entry.path.as_str(),
                entry.kind,
                entry.original_path.as_deref(),
                entry.is_untracked,
                entry.is_conflicted,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("src/a file.ts", "modified", None, false, false),
            ("gone.ts", "deleted", None, false, false),
            ("new.ts", "renamed", Some("old.ts"), false, false),
            ("conflict.ts", "modified", None, false, true),
            ("notes/todo.md", "added", None, true, false),
        ]
    );
}

#[test]
fn porcelain_v2_detached_head() {
    let parsed = parse_status_porcelain("# branch.head (detached)\0");
    assert_eq!(parsed.branch_name, None);
    assert_eq!(parsed.head_ref_type, "detached");
}

#[test]
fn numstat_keeps_insertion_order_and_renames() {
    let stdout = "3\t1\tb.ts\0-\t-\timage.png\0\
                  5\t0\t\0old.ts\0new.ts\0";
    let stats = parse_numstat(stdout);
    let keys: Vec<&str> = stats.0.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(keys, vec!["b.ts", "image.png", "new.ts"]);
    let binary = stats.get("image.png").expect("binary entry");
    assert_eq!((binary.added, binary.removed), (0, 0));
    let renamed = stats.get("new.ts").expect("rename entry");
    assert_eq!(renamed.kind, Some("renamed"));
    assert_eq!(renamed.original_path.as_deref(), Some("old.ts"));
    assert_eq!(kind_from_numstat(stats.get("b.ts").expect("b")), "modified");
}

#[test]
fn remotes_and_tracking_names() {
    assert_eq!(
        parse_remote_list("origin\r\n upstream \n\n"),
        vec!["origin", "upstream"]
    );
    assert_eq!(
        tracking_remote_name(Some("origin/feature/x")).as_deref(),
        Some("origin")
    );
    assert_eq!(tracking_remote_name(Some("")), None);
    assert_eq!(tracking_remote_name(None), None);
}

#[test]
fn index_entries_parse_and_reject_malformed_records() {
    let entries =
        parse_index_entries("100644 abc 0\tsrc/a.ts\x00100755 def 2\tbin/run\0").expect("parse");
    assert_eq!(entries.len(), 2);
    assert_eq!(
        (
            entries[1].mode.as_str(),
            entries[1].object_hash.as_str(),
            entries[1].stage.as_str(),
            entries[1].path.as_str()
        ),
        ("100755", "def", "2", "bin/run")
    );
    assert!(parse_index_entries("100644 abc\tsrc/a.ts\0").is_err());
    assert!(parse_index_entries("no-tab\0").is_err());
}

#[test]
fn branch_refs_sort_current_then_newest_then_name() {
    let stdout = "b\0\0h2\x00100\ncurrent\0origin/current\0h1\x0050\na\0\0h3\x00100\nz\0\0\0\n";
    let branches = parse_branch_refs(stdout, Some("current"), str::cmp);
    let names: Vec<&str> = branches
        .iter()
        .map(|branch| branch["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["current", "a", "b", "z"]);
    assert_eq!(branches[0]["isCurrent"], true);
    assert_eq!(branches[0]["upstreamName"], "origin/current");
    assert_eq!(branches[1]["commitTimestampMs"], 100_000);
    assert_eq!(branches[3]["commitTimestampMs"], serde_json::Value::Null);
}

#[test]
fn graph_records_parse_decorations() {
    let stdout = "h1\0p1 p2\0Ann\x001700000000\0subject\0HEAD -> refs/heads/main, tag: refs/tags/v1, refs/remotes/origin/main\u{1e}\n\
                  h2\0\0\0\0root\0\u{1e}";
    let records = parse_graph_records(stdout);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["parents"], serde_json::json!(["p1", "p2"]));
    assert_eq!(
        records[0]["refs"],
        serde_json::json!([
            {"name": "HEAD", "kind": "head"},
            {"name": "main", "kind": "branch"},
            {"name": "v1", "kind": "tag"},
            {"name": "origin/main", "kind": "remote"},
        ])
    );
    assert_eq!(records[0]["authoredAtMs"], 1_700_000_000_000_i64);
    assert_eq!(records[1]["authorName"], serde_json::Value::Null);
    assert_eq!(records[1]["authoredAtMs"], serde_json::Value::Null);
}

#[test]
fn config_value_handles_missing_and_scoped_output() {
    assert_eq!(
        parse_config_value(&output(Some(1), "", "")).expect("missing"),
        (None, None, None)
    );
    let (scope, source, value) = parse_config_value(&output(
        Some(0),
        "global\tfile:/home/u/.gitconfig\tAnn\tB\n",
        "",
    ))
    .expect("value");
    assert_eq!(scope.as_deref(), Some("global"));
    assert_eq!(source.as_deref(), Some("file:/home/u/.gitconfig"));
    assert_eq!(value.as_deref(), Some("Ann\tB"));
    assert!(parse_config_value(&output(Some(2), "", "boom")).is_err());
}

#[test]
fn branch_mutation_issues_classify_git_errors() {
    let tracked = branch_mutation_issues(&output(
        Some(1),
        "",
        "error: Your local changes to the following files would be overwritten by checkout:\n\ta.ts\n\tb/c.ts\nPlease commit.\n",
    ));
    assert_eq!(tracked[0]["code"], "tracked-changes-would-be-overwritten");
    assert_eq!(tracked[0]["paths"], serde_json::json!(["a.ts", "b/c.ts"]));
    let cases = [
        (
            "fatal: a branch named 'x' already exists",
            "branch-already-exists",
        ),
        ("fatal: invalid reference: nope", "target-branch-not-found"),
        (
            "fatal: 'x' is already used by worktree at '/w'",
            "branch-in-other-worktree",
        ),
        (
            "error: you need to resolve your current index first",
            "conflicts-present",
        ),
        (
            "fatal: cannot switch branch while rebasing",
            "operation-in-progress",
        ),
        ("something else", "unknown"),
    ];
    for (stderr, code) in cases {
        let issues = branch_mutation_issues(&output(Some(128), "", stderr));
        assert_eq!(issues[0]["code"], code, "stderr={stderr}");
        assert_eq!(issues[0]["detail"], stderr);
    }
    assert_eq!(
        invalid_branch_name_issue(Some("  ")),
        serde_json::json!({"code": "invalid-branch-name", "message": "Branch name is invalid.", "detail": null})
    );
}
