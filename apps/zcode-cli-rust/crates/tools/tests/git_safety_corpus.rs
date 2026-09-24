//! 差分：Rust `bash_git_safety::is_runtime_context_unsafe` 与 TS `isGitRuntimeContextUnsafe`。
//! 语料由 scripts/generate-zcode-cli-rust-git-safety-corpus.mjs 生成：每例描述一棵目录树，
//! 这里重建同构目录后比对结论。symlink 用例在无权限的平台上跳过。
use serde_json::Value;
use std::path::{Path, PathBuf};
use zcode_cli_tools::bash_git_safety::is_runtime_context_unsafe;

/// 每个用例独占一个临时目录；本 crate 不引入 tempfile，避免为测试新增依赖。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("zcode-git-safety-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn materialize(root: &Path, entries: &[Value]) -> bool {
    for entry in entries {
        let path = root.join(entry[0].as_str().unwrap());
        let kind = entry[1].as_str().unwrap();
        let value = entry[2].as_str().unwrap();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let ok = match kind {
            "dir" => std::fs::create_dir_all(&path).is_ok(),
            "file" => std::fs::write(&path, value).is_ok(),
            "symlink" => {
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(value, &path).is_ok()
                }
                #[cfg(windows)]
                {
                    std::os::windows::fs::symlink_dir(value, &path).is_ok()
                }
            }
            other => panic!("unknown entry kind {other}"),
        };
        // 平台限制（symlink 权限、保留设备名等）导致建树失败时跳过该用例，
        // 由生成机上的 TS 断言兜底，不让环境怪癖伪装成判定差异。
        if !ok {
            return false;
        }
    }
    true
}

#[test]
fn rust_git_safety_matches_ts_corpus() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/git_safety_corpus.json")).unwrap();
    let mut checked = 0;
    let mut skipped: Vec<String> = vec![];
    let mut failures = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let root = TempDir::new(name);
        if !materialize(root.path(), case["entries"].as_array().unwrap()) {
            skipped.push(name.to_owned());
            continue;
        }
        if case["expect"].is_null() {
            // 结论依赖 %TEMP% 之上是否存在仓库，不做跨机断言。
            continue;
        }
        let cwd = root.path().join(case["cwd"].as_str().unwrap());
        std::fs::create_dir_all(&cwd).unwrap();
        let want = case["expect"].as_i64().unwrap() == 1;
        let got = is_runtime_context_unsafe(Some(&cwd));
        checked += 1;
        if got != want {
            failures.push(format!("{name}: rust {got} != ts {want}"));
        }
    }
    assert!(
        checked >= 10,
        "too few cases checked: {checked}, skipped: {skipped:?}"
    );
    assert!(failures.is_empty(), "{} mismatches: {failures:?}", failures.len());
}
