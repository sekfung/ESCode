//! profile 单测。kitty 用例的期望值来自 Node 对 `/^\s*font_family\s+(.+)$/m` 的实测输出。

use super::super::jsonc::parse_jsonc;
use super::*;

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn kitty_match_follows_js_regex_semantics() {
    let cases: [(&str, Option<&str>); 11] = [
        ("font_family  JetBrains Mono\n", Some("JetBrains Mono")),
        (
            "# c\n  font_family \"Fira Code\"\nfont_size 12",
            Some("\"Fira Code\""),
        ),
        ("font_family   \nfoo", Some("foo")),
        ("font_family  ", Some(" ")),
        ("font_family\n\nX Y\nz", Some("X Y")),
        ("xfont_family a\nfont_family b", Some("b")),
        ("font_family\tTab\r\nnext", Some("Tab")),
        ("\n\n   font_family  A  \n", Some("A  ")),
        ("font_family", None),
        ("font_familyX y", None),
        ("font_family\u{2028}\u{2028}q", Some("q")),
    ];
    for (input, expected) in cases {
        assert_eq!(
            kitty_font_family_match(input).as_deref(),
            expected,
            "input={input:?}"
        );
    }
}

#[test]
fn jsonc_accepts_comments_and_trailing_commas_but_only_objects() {
    let relaxed = "{\n // line\n \"a\": [1, 2,], /* block */\n \"url\": \"http://x//y\",\n}";
    let parsed = parse_jsonc(relaxed).expect("relaxed object");
    assert_eq!(parsed["a"], serde_json::json!([1, 2]));
    assert_eq!(parsed["url"], "http://x//y", "字符串内的 // 不是注释");
    assert!(parse_jsonc("[1]").is_none());
    assert!(
        parse_jsonc("\u{feff}{}").is_none(),
        "BOM 与 JSON.parse 一样不接受"
    );
    assert!(parse_jsonc("{").is_none());
}

#[test]
fn dedupe_appends_missing_fallbacks_in_order() {
    let stack = dedupe_font_family_stack("  Fira Code , Menlo,, ");
    assert!(stack.starts_with("Fira Code, Menlo, ui-monospace, SFMono-Regular, SF Mono, Monaco,"));
    assert_eq!(stack.matches("Menlo").count(), 1);
    assert!(stack.ends_with(", monospace"));
}

#[tokio::test]
async fn profile_precedence_custom_then_system_then_fallback() {
    let home = tempfile::tempdir().expect("home");
    let kitty = home.path().join(".config").join("kitty");
    tokio::fs::create_dir_all(&kitty).await.expect("mkdir");
    tokio::fs::write(kitty.join("kitty.conf"), "font_family  \"Kitty Mono\"\n")
        .await
        .expect("write");
    let vars = env(&[("HOME", &home.path().to_string_lossy())]);

    let system = resolve_font_profile(None, true, &vars, "linux").await;
    assert_eq!(system.source, "system");
    assert!(system.font_family.starts_with("Kitty Mono, ui-monospace"));

    let custom = resolve_font_profile(Some(" Custom "), true, &vars, "linux").await;
    assert_eq!(
        (custom.source, custom.font_family.split(',').next()),
        ("custom", Some("Custom"))
    );

    let not_inheriting = resolve_font_profile(None, false, &vars, "linux").await;
    assert_eq!(not_inheriting.source, "fallback");
    assert_eq!(not_inheriting.font_family, FONT_FAMILY_FALLBACKS.join(", "));

    let windows_skips_kitty = resolve_font_profile(None, true, &vars, "win32").await;
    assert_eq!(
        windows_skips_kitty.source, "fallback",
        "kitty 检测器不适用于 win32"
    );
}

#[tokio::test]
async fn vscode_beats_kitty_and_windows_terminal_prefers_default_profile() {
    let home = tempfile::tempdir().expect("home");
    let vscode = home.path().join(".config").join("Code").join("User");
    tokio::fs::create_dir_all(&vscode).await.expect("mkdir");
    tokio::fs::write(
        vscode.join("settings.json"),
        "{ // c\n \"terminal.integrated.fontFamily\": \"Code Mono\", }",
    )
    .await
    .expect("write");
    let kitty = home.path().join(".config").join("kitty");
    tokio::fs::create_dir_all(&kitty).await.expect("mkdir");
    tokio::fs::write(kitty.join("kitty.conf"), "font_family K\n")
        .await
        .expect("write");
    let vars = env(&[("HOME", &home.path().to_string_lossy())]);
    let detected = detect_system_profile(&vars, "linux")
        .await
        .expect("detected");
    assert_eq!(detected.font_family.as_deref(), Some("Code Mono"));

    let local = tempfile::tempdir().expect("local");
    let wt = local.path().join("Microsoft").join("Windows Terminal");
    tokio::fs::create_dir_all(&wt).await.expect("mkdir");
    let settings = r#"{"defaultProfile":"{b}","profiles":{"defaults":{"font":{"face":"Defaults"}},
        "list":[{"guid":"{a}","font":{"face":"A"}},{"guid":"{b}","font":{"face":"B"}}]}}"#;
    tokio::fs::write(wt.join("settings.json"), settings)
        .await
        .expect("write");
    let wt_env = env(&[
        ("LOCALAPPDATA", &local.path().to_string_lossy()),
        ("HOME", "/nonexistent"),
    ]);
    let found = detect_system_profile(&wt_env, "win32").await.expect("wt");
    assert_eq!(
        found.font_family.as_deref(),
        Some("B"),
        "默认 profile 的字体优先于 defaults"
    );
}
