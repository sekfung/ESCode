//! `gen-ui.readDocument` 的受限读取（`rust-u2-gen-ui.md` §3.1/§6）：真临时目录 + 真符号链接。
use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("visualizations");
        std::fs::create_dir_all(root.join("scope")).unwrap();
        Self { _dir: dir, root }
    }

    fn target(&self, file: &Path) -> serde_json::Value {
        serde_json::json!({
            "workspacePath": "/workspace",
            "sessionId": "s1",
            "path": file.to_string_lossy(),
        })
    }

    fn root_str(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

#[test]
fn reads_a_fragment_from_the_output_root() {
    let fixture = Fixture::new();
    let file = fixture.root.join("scope/report.html");
    write(&file, "<div id=\"chart\">hi</div>");
    let document = read_document(&fixture.target(&file), &fixture.root_str()).unwrap();
    assert_eq!(document.html, "<div id=\"chart\">hi</div>");
    assert_eq!(document.path, file.to_string_lossy());
}

#[test]
fn rejects_paths_outside_the_output_root_and_relative_paths() {
    let fixture = Fixture::new();
    let outside = fixture._dir.path().join("outside.html");
    write(&outside, "<p>out</p>");
    let error = read_document(&fixture.target(&outside), &fixture.root_str()).unwrap_err();
    assert_eq!(error, "Gen UI file is outside the host output directory");

    // 根自身不是文件级的合法目标（TS 的 `!child` 分支）。
    let error = read_document(&fixture.target(&fixture.root), &fixture.root_str()).unwrap_err();
    assert_ne!(error, "");

    // 相对路径过不了路径 schema（读取范围不由它决定）。
    let relative = serde_json::json!({
        "workspacePath": "/workspace", "sessionId": "s1", "path": "visualizations/report.html"
    });
    assert!(read_document(&relative, &fixture.root_str()).is_err());
}

#[cfg(unix)]
#[test]
fn rejects_symlinks_at_every_level_including_the_last_segment() {
    let fixture = Fixture::new();
    let real = fixture._dir.path().join("real.html");
    write(&real, "<p>secret</p>");

    // 末段是符号链接。
    let last = fixture.root.join("scope/link.html");
    std::os::unix::fs::symlink(&real, &last).unwrap();
    let error = read_document(&fixture.target(&last), &fixture.root_str()).unwrap_err();
    assert_eq!(error, "Gen UI does not read symbolic links");

    // 中间段是符号链接（指向根外的目录）。
    let outside_dir = fixture._dir.path().join("outside-dir");
    std::fs::create_dir_all(&outside_dir).unwrap();
    write(&outside_dir.join("report.html"), "<p>out</p>");
    let middle = fixture.root.join("linked");
    std::os::unix::fs::symlink(&outside_dir, &middle).unwrap();
    let error = read_document(
        &fixture.target(&middle.join("report.html")),
        &fixture.root_str(),
    )
    .unwrap_err();
    assert_eq!(error, "Gen UI does not read symbolic links");
}

#[test]
fn rejects_directories_oversized_files_and_full_documents() {
    let fixture = Fixture::new();
    // 目录（名字合法但类型不对）。
    let dir = fixture.root.join("scope/dir.html");
    std::fs::create_dir_all(&dir).unwrap();
    let error = read_document(&fixture.target(&dir), &fixture.root_str()).unwrap_err();
    assert_eq!(error, "Gen UI requires a regular file under 5 MB");

    // 恰好 5 MB 允许；多 1 字节拒绝。
    let exact = fixture.root.join("scope/exact.html");
    write(&exact, &"x".repeat(HTML_MAX_BYTES));
    assert!(read_document(&fixture.target(&exact), &fixture.root_str()).is_ok());
    // 超限文件在**读取前**的 size 检查就被拦下（TS 同序：`before.size > MAX` 先于读取循环）——
    // 「file exceeds 5 MB」只属于"打开后文件又长大"的竞态路径，静态文件拿不到那条文案。
    let over = fixture.root.join("scope/over.html");
    write(&over, &"x".repeat(HTML_MAX_BYTES + 1));
    let error = read_document(&fixture.target(&over), &fixture.root_str()).unwrap_err();
    assert_eq!(error, "Gen UI requires a regular file under 5 MB");

    // 完整文档（含 doctype / html / head / body 之一）不算片段；`<htmlx` 不算命中。
    let full = fixture.root.join("scope/full.html");
    write(&full, "<!DOCTYPE html><html><body>hi</body></html>");
    let error = read_document(&fixture.target(&full), &fixture.root_str()).unwrap_err();
    assert_eq!(error, "Gen UI requires an HTML fragment");
    for body in ["<body>hi</body>", "<HEAD></HEAD>", "<html>", "</div>ok"] {
        write(&full, body);
        let outcome = read_document(&fixture.target(&full), &fixture.root_str());
        if body == "</div>ok" {
            assert!(outcome.is_ok(), "闭合标签不是文档标记: {body}");
        } else {
            assert_eq!(outcome.unwrap_err(), "Gen UI requires an HTML fragment");
        }
    }
    write(&full, "<htmlx>hi</htmlx>");
    assert!(
        read_document(&fixture.target(&full), &fixture.root_str()).is_ok(),
        "`<htmlx` 不满足 `html(?:\\s|>)`，不算文档标记"
    );
    // 大小写不敏感 + `blank` 后接空白也命中。
    write(&full, "<!doctype  html>");
    assert_eq!(
        read_document(&fixture.target(&full), &fixture.root_str()).unwrap_err(),
        "Gen UI requires an HTML fragment"
    );
}

#[test]
fn rejects_invalid_utf8_and_missing_files() {
    let fixture = Fixture::new();
    let file = fixture.root.join("scope/report.html");
    std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
    let error = read_document(&fixture.target(&file), &fixture.root_str()).unwrap_err();
    assert!(error.contains("valid UTF-8"), "{error}");

    // 文件不存在：逐级 lstat 就失败（不回"片段"之类的语义错误）。
    let missing = fixture.root.join("scope/missing.html");
    assert!(read_document(&fixture.target(&missing), &fixture.root_str()).is_err());
}

/// 片段校验自身也有上限（`assertGenUiFragment` 的 byteLength 分支）——它在读取器里被 size 检查
/// 前置挡住，只能直接测。
#[test]
fn fragment_check_caps_at_five_megabytes_on_its_own() {
    assert!(assert_fragment(&"x".repeat(HTML_MAX_BYTES)).is_ok());
    assert_eq!(
        assert_fragment(&"x".repeat(HTML_MAX_BYTES + 1)).unwrap_err(),
        "Gen UI file exceeds 5 MB"
    );
}

fn write(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
}
