use super::*;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: std::collections::BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_lowercase(), (*v).to_owned()))
        .collect();
    move |key| map.get(&key.to_lowercase()).cloned()
}

#[test]
fn windows_install_dirs_are_forbidden_including_children_and_case_variants() {
    let env = env_of(&[
        ("ProgramFiles", "C:\\Program Files"),
        ("LOCALAPPDATA", "C:\\Users\\u\\AppData\\Local"),
    ]);
    assert_eq!(
        forbidden_install_dir("c:/program files/escode/data", true, &env).as_deref(),
        Some("C:\\Program Files\\ESCode")
    );
    assert!(
        forbidden_install_dir("C:\\Users\\u\\AppData\\Local\\Programs\\ESCode", true, &env)
            .is_some()
    );
    assert!(
        forbidden_install_dir("C:\\Program Files\\ESCode2", true, &env).is_none(),
        "前缀相同但不是子目录"
    );
    assert!(forbidden_install_dir("D:\\data", true, &env).is_none());
    assert!(
        forbidden_install_dir("C:\\Program Files\\ESCode", false, &env).is_none(),
        "非 Windows 不校验"
    );
    assert!(
        forbidden_install_dir("C:\\Program Files\\x\\..\\ESCode\\y", true, &env).is_some(),
        "先规范化 .."
    );
    let custom = env_of(&[("ESCODE_WINDOWS_APP_INSTALL_DIR", "E:\\Apps\\ESCode\\")]);
    assert!(forbidden_install_dir("E:\\apps\\escode", true, &custom).is_some());
}

#[tokio::test]
async fn default_project_reports_whether_it_was_created() {
    let dir = tempfile::tempdir().expect("dir");
    let (path, created) = ensure_default_project(dir.path()).await.expect("first");
    assert!(created && path.ends_with(DEFAULT_PROJECT_NAME) && path.is_dir());
    assert!(!ensure_default_project(dir.path()).await.expect("second").1);
}

#[test]
fn copy_skips_setting_files_symlinks_and_never_overwrites() {
    let dir = tempfile::tempdir().expect("dir");
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    let v2 = old.join(".escode").join("v2");
    std::fs::create_dir_all(v2.join("sub")).expect("mkdir");
    std::fs::write(v2.join("a.json"), "old-a").expect("write");
    std::fs::write(v2.join("sub").join("b.txt"), "b").expect("write");
    std::fs::write(v2.join("setting.json"), "boot").expect("write");
    std::fs::write(v2.join("setting.json.lock"), "lock").expect("write");
    #[cfg(unix)]
    std::os::unix::fs::symlink(v2.join("a.json"), v2.join("link.json")).expect("symlink");
    let target = new.join(".escode").join("v2");
    std::fs::create_dir_all(&target).expect("mkdir");
    std::fs::write(target.join("a.json"), "keep-me").expect("write");
    copy_data_directory(&old, &new).expect("copy");
    assert_eq!(
        std::fs::read_to_string(target.join("a.json")).expect("read"),
        "keep-me"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("sub").join("b.txt")).expect("read"),
        "b"
    );
    assert!(!target.join("setting.json").exists() && !target.join("setting.json.lock").exists());
    assert!(!target.join("link.json").exists());
    assert!(
        copy_data_directory(&dir.path().join("missing"), &new).is_err(),
        "源不存在 → ENOENT 上抛"
    );
}
