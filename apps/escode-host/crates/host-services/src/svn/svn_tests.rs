//! svn 解析、路径与错误归类的单元测试（无外部进程）。

use super::config::{compare_versions, extract_error_codes, normalize_svn_path, parse_version};
use super::errors::{classify, summarize};
use super::parse::{
    count_diff_lines, count_updated_paths, item_status, parse_blame, parse_committed_revision,
    parse_info, parse_list, parse_log, parse_mergeinfo, parse_status,
};
use super::paths::{
    build_unversioned_patch, derive_branch_ref, is_safe_relative_path, local_target,
    read_unversioned_file_stat, relative_path, relative_repository_path, split_diff_by_index,
    to_workspace_relative,
};
use super::xml::{child, parse};

const INFO_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<info>
<entry kind="dir" path="." revision="12">
<url>file:///repo/trunk</url>
<repository><root>file:///repo</root><uuid>uuid-1</uuid></repository>
<wc-info><wcroot-abspath>/tmp/wc</wcroot-abspath></wc-info>
<commit revision="10"><author>alice</author></commit>
</entry>
<entry kind="file" path="src/a.ts" revision="5">
<url>file:///repo/trunk/src/a.ts</url>
<repository><root>file:///repo</root><uuid>uuid-1</uuid></repository>
<wc-info><wcroot-abspath>/tmp/wc</wcroot-abspath></wc-info>
<conflict type="text"><prev-base-file>/tmp/wc/src/a.ts.r4</prev-base-file></conflict>
</entry>
</info>"#;

#[test]
fn parses_info_entries_and_conflicts() {
    let entries = parse_info(INFO_XML);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, ".");
    assert_eq!(entries[0].revision, Some(12));
    assert_eq!(entries[0].url.as_deref(), Some("file:///repo/trunk"));
    assert_eq!(entries[0].repository_root.as_deref(), Some("file:///repo"));
    assert_eq!(entries[0].repository_uuid.as_deref(), Some("uuid-1"));
    assert_eq!(entries[0].wc_root.as_deref(), Some("/tmp/wc"));
    assert!(entries[0].conflict.is_none());
    let conflict = entries[1].conflict.as_ref().expect("text conflict");
    assert_eq!(conflict.kinds, vec!["text"]);
    assert_eq!(
        conflict.artifact_paths,
        vec!["/tmp/wc/src/a.ts.r4".to_owned()]
    );
}

#[test]
fn parses_status_and_external_scope() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<status>
<target path=".">
<entry path="m.txt"><wc-status item="modified" props="none" revision="3"/></entry>
<entry path="p.txt"><wc-status item="normal" props="modified" revision="3"/></entry>
<entry path="c.txt"><wc-status item="conflicted" props="none" tree-conflicted="true" revision="3"/></entry>
<entry path="i.log"><wc-status item="ignored" props="none"/></entry>
<entry path="ext"><wc-status item="external" props="none"/></entry>
<entry path="ext/x.txt"><wc-status item="modified" props="none"/></entry>
</target>
</status>"#;
    let entries = parse_status(xml);
    assert_eq!(entries[0].path, "m.txt");
    assert_eq!(item_status(&entries[0]), Some("modified"));
    assert_eq!(item_status(&entries[1]), Some("props-modified"));
    assert_eq!(item_status(&entries[2]), Some("conflicted"));
    assert_eq!(item_status(&entries[3]), None);
    assert_eq!(item_status(&entries[4]), Some("external"));
    assert!(!entries[4].in_external);
    assert!(entries[5].in_external);
}

#[test]
fn parses_log_entries_with_changed_paths() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<log>
<logentry revision="7">
<author>alice</author>
<date>2026-10-20T10:00:00.000000Z</date>
<paths>
<path action="M" kind="file">/trunk/a.txt</path>
<path action="A" kind="dir" copyfrom-path="/branches/x" copyfrom-rev="4">/trunk/d</path>
</paths>
<msg>fix: 中文提交信息</msg>
</logentry>
</log>"#;
    let entries = parse_log(xml);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].revision, 7);
    assert_eq!(entries[0].author, "alice");
    assert_eq!(entries[0].message, "fix: 中文提交信息");
    assert_eq!(entries[0].changed_paths[0].action, "M");
    assert_eq!(entries[0].changed_paths[1].action, "A");
    assert_eq!(
        entries[0].changed_paths[1].copy_from_path.as_deref(),
        Some("/branches/x")
    );
    assert_eq!(entries[0].changed_paths[1].copy_from_revision, Some(4));
}

#[test]
fn parses_list_entries() {
    let xml = r#"<lists><list path="file:///repo">
<entry kind="dir"><name>trunk</name></entry>
<entry kind="dir"><name>branches</name></entry>
</list></lists>"#;
    let entries = parse_list(xml);
    assert_eq!(entries[0].name, "trunk");
    assert_eq!(entries[0].kind, "dir");
}

#[test]
fn xml_decodes_entities_and_ignores_comments() {
    let root = parse(
        r#"<!-- c --><root a="x &amp; y"><msg>&lt;hi&gt; &#228; &#x4e2d;</msg><empty/></root>"#,
    )
    .expect("root");
    assert_eq!(root.name, "root");
    assert_eq!(root.attrs.get("a").map(String::as_str), Some("x & y"));
    assert_eq!(
        child(Some(&root), "msg").map(|node| node.text.clone()),
        Some("<hi> ä 中".to_owned())
    );
}

#[test]
fn counts_updated_paths_and_revisions() {
    let stdout = "Updating '.':\nU    a.txt\nA    b.txt\n   C c.txt\nUpdated to revision 5.\n";
    assert_eq!(count_updated_paths(stdout), 3);
    assert_eq!(parse_committed_revision("Committed revision 12."), Some(12));
    assert_eq!(parse_committed_revision("Committed revision 3"), Some(3));
    assert_eq!(parse_committed_revision("no revision"), None);
}

#[test]
fn counts_diff_lines_and_binary_marker() {
    let patch = "Index: a.txt\n===================================================================\n--- a.txt\t(revision 1)\n+++ a.txt\t(working copy)\n@@ -1 +1,2 @@\n-old\n+new\n+more\n";
    let count = count_diff_lines(patch);
    assert_eq!((count.additions, count.deletions), (2, 1));
    assert!(!count.is_binary);
    let binary = "Index: b.bin\n===================================================================\nCannot display: file marked as a binary type.\nsvn:mime-type = application/octet-stream\n";
    assert!(count_diff_lines(binary).is_binary);
}

#[test]
fn classifies_errors_by_code_only() {
    assert_eq!(
        classify("svn: E155004: working copy locked"),
        "working-copy-locked"
    );
    assert_eq!(classify("svn: E155011: out of date"), "out-of-date");
    assert_eq!(
        classify("svn: E170001: authorization failed"),
        "auth-required"
    );
    assert_eq!(classify("svn: E170013: unable to connect"), "unknown");
    assert_eq!(
        classify("svn: E170013: unable to connect\nsvn: E170001: authorization failed"),
        "auth-required"
    );
    assert_eq!(classify("svn: E155015: conflict"), "unresolved-conflicts");
    assert_eq!(
        classify("svn: E155007: not a working copy"),
        "not-working-copy"
    );
    assert_eq!(classify("boom"), "unknown");
    assert_eq!(
        summarize("noise\nsvn: E155004: locked\nmore"),
        "svn: E155004: locked"
    );
}

#[test]
fn config_helpers_match_ts_semantics() {
    assert_eq!(
        extract_error_codes("E155004 E170001 xE1"),
        vec!["E155004", "E170001"]
    );
    assert_eq!(parse_version("1.14.3\n"), Some("1.14.3".to_owned()));
    assert_eq!(parse_version("garbage"), None);
    assert!(compare_versions("1.14.3", "1.8.0").is_gt());
    assert!(compare_versions("1.7.9", "1.8.0").is_lt());
    assert_eq!(normalize_svn_path(".\\a\\b"), "a/b");
}

#[test]
fn derives_branch_refs_from_relative_paths() {
    assert_eq!(derive_branch_ref("trunk").kind, "trunk");
    assert_eq!(derive_branch_ref("/trunk/sub").kind, "trunk");
    let branch = derive_branch_ref("branches/feature-x/sub");
    assert_eq!((branch.kind, branch.name.as_str()), ("branch", "feature-x"));
    assert_eq!(branch.relative_path, "branches/feature-x");
    let tag = derive_branch_ref("tags/v1.0");
    assert_eq!((tag.kind, tag.name.as_str()), ("tag", "v1.0"));
    let other = derive_branch_ref("project/trunk");
    assert_eq!(other.kind, "other");
    assert_eq!(other.relative_path, "project/trunk");
}

#[test]
fn computes_relative_repository_paths() {
    assert_eq!(
        relative_repository_path("file:///repo/trunk/a%20b", "file:///repo/"),
        Some("trunk/a b".to_owned())
    );
    assert_eq!(
        relative_repository_path("file:///repo2/trunk", "file:///repo"),
        None
    );
}

#[test]
fn path_helpers_guard_workspace_escape() {
    assert_eq!(local_target("icon@2x.png"), "icon@2x.png@");
    assert_eq!(local_target("a.txt"), "a.txt");
    assert!(is_safe_relative_path("src/a.ts"));
    assert!(!is_safe_relative_path("../a.ts"));
    assert!(!is_safe_relative_path("/abs/a.ts"));
    assert!(!is_safe_relative_path(""));
    assert_eq!(
        to_workspace_relative("/tmp/wc", "/tmp/wc/src/a.ts"),
        "src/a.ts"
    );
    assert_eq!(to_workspace_relative("/tmp/wc", "/tmp/wc/"), ".");
    assert_eq!(relative_path("/tmp/wc", "/tmp/wc/sub"), "sub");
    assert_eq!(relative_path("/a/b", "/a/c/d"), "../c/d");
}

#[test]
fn splits_diff_sections_by_index_header() {
    let patch = "Index: a.txt\n--- a\n+++ a\n@@ -1 +1 @@\n-x\n+y\nIndex: b.txt\n--- b\n+++ b\n";
    let sections = split_diff_by_index(patch);
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].0, "a.txt");
    assert_eq!(sections[1].0, "b.txt");
    assert!(sections[1].1.contains("Index: b.txt"));
}

#[test]
fn credential_flags_keep_passwords_out_of_argv() {
    use super::Credentials;
    use super::runtime::with_credentials;

    let credentials = || Credentials {
        username: "alice".to_owned(),
        password: "s3cret".to_owned(),
        store_in_cache: false,
    };
    let (args, stdin) = with_credentials(
        vec![
            "--non-interactive".to_owned(),
            "update".to_owned(),
            ".".to_owned(),
        ],
        Some(credentials()),
        true,
    );
    assert_eq!(stdin.as_deref(), Some("s3cret"));
    assert!(!args.iter().any(|arg| arg == "s3cret"));
    assert!(args.contains(&"--password-from-stdin".to_owned()));
    assert!(args.contains(&"--username".to_owned()));
    assert!(args.contains(&"--no-auth-cache".to_owned()));
    assert_eq!(args[0], "--no-auth-cache");
    assert_eq!(args[1], "--password-from-stdin");
    assert_eq!(args[2], "--username");
    assert_eq!(args[3], "alice");

    // svn < 1.10 回退到 --password，且 no-auth-cache 可关闭。
    let (args, stdin) = with_credentials(
        vec!["--non-interactive".to_owned(), "log".to_owned()],
        Some(Credentials {
            username: "bob".to_owned(),
            password: "pw".to_owned(),
            store_in_cache: true,
        }),
        false,
    );
    assert!(stdin.is_none());
    assert!(args.contains(&"--password".to_owned()));
    assert!(args.contains(&"pw".to_owned()));
    assert!(!args.contains(&"--no-auth-cache".to_owned()));
    assert!(!args.contains(&"--password-from-stdin".to_owned()));

    let (args, stdin) = with_credentials(vec!["info".to_owned()], None, true);
    assert!(stdin.is_none());
    assert_eq!(args, vec!["info".to_owned()]);
}

#[test]
fn parses_blame_lines_and_truncates() {
    let xml = r#"<blame><target path="a.txt">
<entry line-number="1"><commit revision="3"><author>alice</author></commit><token>alpha</token></entry>
<entry line-number="2"><commit revision="3"><author>alice</author></commit><token>beta</token></entry>
</target></blame>"#;
    let (lines, truncated) = parse_blame(xml, 10);
    assert!(!truncated);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].line_number, 1);
    assert_eq!(lines[0].revision, 3);
    assert_eq!(lines[0].author, "alice");
    assert_eq!(lines[0].content, "alpha");
    let (lines, truncated) = parse_blame(xml, 1);
    assert!(truncated);
    assert_eq!(lines.len(), 1);
}

#[test]
fn parses_mergeinfo_targets() {
    let xml = r#"<properties>
<target path="file:///repo/trunk"><property name="svn:mergeinfo">/branches/feature:3-4</property></target>
<target path="file:///repo/branches/feature"><property name="other">x</property></target>
</properties>"#;
    let entries = parse_mergeinfo(xml);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "file:///repo/trunk");
    assert_eq!(entries[0].mergeinfo, "/branches/feature:3-4");
}

#[tokio::test]
async fn builds_unversioned_patch_and_stats() {
    let dir = tempfile::tempdir().expect("tempdir");
    let text_path = dir.path().join("new.txt");
    tokio::fs::write(&text_path, "one\ntwo\n")
        .await
        .expect("write");
    let patch = build_unversioned_patch(&text_path.to_string_lossy(), "new.txt").await;
    assert!(!patch.is_binary);
    assert!(!patch.truncated);
    let patch = patch.patch.expect("patch");
    assert!(patch.starts_with("Index: new.txt"));
    assert!(patch.contains("@@ -0,0 +1,2 @@"));
    assert!(patch.contains("+one\n+two"));
    let stat = read_unversioned_file_stat(&text_path.to_string_lossy()).await;
    assert_eq!(stat.additions, 2);
    assert!(!stat.is_binary);
    assert!(!stat.is_directory);

    let binary_path = dir.path().join("new.bin");
    tokio::fs::write(&binary_path, [0u8, 1, 2])
        .await
        .expect("write");
    let binary = build_unversioned_patch(&binary_path.to_string_lossy(), "new.bin").await;
    assert!(binary.is_binary);
    assert!(binary.patch.is_none());
    assert!(
        read_unversioned_file_stat(&binary_path.to_string_lossy())
            .await
            .is_binary
    );
}
