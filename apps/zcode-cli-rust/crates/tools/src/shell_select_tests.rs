use super::*;

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}

fn only<'a>(paths: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
    move |p| paths.contains(&p)
}

#[test]
fn windows_prefers_fixed_git_bash_path() {
    let got = resolve(
        Platform::Windows,
        &env(&[]),
        None,
        &only(&["C:\\Program Files\\Git\\bin\\bash.exe"]),
    );
    assert_eq!(got.dialect, Dialect::GitBash);
    assert_eq!(got.source, Source::AutoDetected);
    assert_eq!(
        got.path.as_deref(),
        Some("C:\\Program Files\\Git\\bin\\bash.exe")
    );
}

#[test]
fn windows_infers_git_bash_from_git_on_path_case_insensitive_env() {
    let got = resolve(
        Platform::Windows,
        &env(&[("Path", "D:\\tools\\Git\\cmd;"), ("PATHEXT", ".EXE")]),
        None,
        &only(&[
            "D:\\tools\\Git\\cmd\\git.exe",
            "D:\\tools\\Git\\bin\\bash.exe",
        ]),
    );
    assert_eq!(got.path.as_deref(), Some("D:\\tools\\Git\\bin\\bash.exe"));
}

#[test]
fn windows_infers_from_mingw_git_two_levels_up() {
    let got = resolve(
        Platform::Windows,
        &env(&[("PATH", "E:\\Git\\mingw64\\bin")]),
        None,
        &only(&["E:\\Git\\mingw64\\bin\\git.exe", "E:\\Git\\bin\\bash.exe"]),
    );
    assert_eq!(got.path.as_deref(), Some("E:\\Git\\bin\\bash.exe"));
}

#[test]
fn windows_without_git_bash_falls_back_to_legacy_comspec() {
    let e = env(&[("COMSPEC", "C:\\Windows\\system32\\cmd.exe")]);
    let got = resolve(Platform::Windows, &e, None, &only(&[]));
    assert_eq!(got.dialect, Dialect::Legacy);
    let plan = spawn_plan(Platform::Windows, &e, &got, "dir");
    assert_eq!(plan.file, "C:\\Windows\\system32\\cmd.exe");
    assert_eq!(plan.args, ["/d", "/s", "/c", "dir"]);
}

#[test]
fn windows_user_cmd_override_beats_auto_git_bash_without_precheck() {
    let over = Override {
        dialect: Dialect::Cmd,
        path: Some("cmd.exe".into()),
    };
    let got = resolve(
        Platform::Windows,
        &env(&[]),
        Some(&over),
        &only(&["C:\\Program Files\\Git\\bin\\bash.exe"]),
    );
    assert_eq!(got.dialect, Dialect::Cmd);
    assert_eq!(got.source, Source::UserConfig);
}

#[test]
fn windows_missing_git_bash_override_falls_back_to_auto() {
    let over = Override {
        dialect: Dialect::GitBash,
        path: Some("Z:\\missing\\bash.exe".into()),
    };
    let got = resolve(
        Platform::Windows,
        &env(&[]),
        Some(&over),
        &only(&["C:\\Program Files (x86)\\Git\\bin\\bash.exe"]),
    );
    assert_eq!(got.source, Source::AutoDetected);
    assert_eq!(
        got.path.as_deref(),
        Some("C:\\Program Files (x86)\\Git\\bin\\bash.exe")
    );
}

#[test]
fn posix_uses_shell_env_first_then_bash_order() {
    let got = resolve(
        Platform::Posix,
        &env(&[("SHELL", "/usr/local/bin/bash"), ("PATH", "/usr/bin")]),
        None,
        &only(&["/usr/local/bin/bash", "/usr/bin/zsh"]),
    );
    assert_eq!(got.path.as_deref(), Some("/usr/local/bin/bash"));
}

#[test]
fn posix_defaults_to_zsh_before_bash() {
    let got = resolve(
        Platform::Posix,
        &env(&[("SHELL", "/bin/fish"), ("PATH", "/usr/bin/")]),
        None,
        &only(&["/bin/bash", "/usr/bin/zsh"]),
    );
    assert_eq!(got.path.as_deref(), Some("/usr/bin/zsh"));
    let plan = spawn_plan(Platform::Posix, &[], &got, "echo hi");
    assert_eq!(plan.args, ["-c", "-l", "echo hi"]);
    assert!(
        plan.env_overlay
            .contains(&("GIT_EDITOR".into(), "true".into()))
    );
    assert!(
        plan.env_overlay
            .contains(&("SHELL".into(), "/usr/bin/zsh".into()))
    );
}

#[test]
fn posix_without_bash_or_zsh_uses_bin_sh() {
    let got = resolve(Platform::Posix, &env(&[]), None, &only(&[]));
    let plan = spawn_plan(Platform::Posix, &[], &got, "true");
    assert_eq!(plan.file, "/bin/sh");
    assert_eq!(plan.args, ["-c", "true"]);
}

#[test]
fn win_normalize_matches_node_win32() {
    assert_eq!(
        win_normalize("C:\\a\\cmd\\..\\bin\\bash.exe"),
        "C:\\a\\bin\\bash.exe"
    );
    assert_eq!(win_normalize("C:/a//b/./c"), "C:\\a\\b\\c");
}
