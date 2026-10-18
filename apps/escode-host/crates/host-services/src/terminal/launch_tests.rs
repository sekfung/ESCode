//! 启动参数解析单测（期望值逐条对照 TS `resolveTerminalEnv` / `resolveTerminalShell` / `resolveTerminalCwd`）。

use super::*;

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn terminal_env_forces_interactive_terminal_vars() {
    let result = terminal_env(
        &env(&[("TERM", "dumb"), ("CI", "1"), ("COLORTERM", "  24bit ")]),
        false,
    );
    assert_eq!(result["TERM"], "xterm-256color");
    assert_eq!(result["COLORTERM"], "24bit");
    assert!(
        !result.contains_key("CI"),
        "dumb 终端采集环境带来的 CI=1 必须去掉"
    );
    let kept = terminal_env(&env(&[("CI", "1"), ("TERM", "xterm")]), false);
    assert_eq!(kept["CI"], "1", "只有原 TERM=dumb 才删除 CI");
    assert_eq!(
        terminal_env(&env(&[("COLORTERM", "  ")]), false)["COLORTERM"],
        "truecolor"
    );
}

#[test]
fn terminal_env_fills_utf8_locale_only_when_missing_or_c() {
    let missing = terminal_env(&env(&[]), false);
    assert_eq!(
        (missing["LANG"].as_str(), missing["LC_CTYPE"].as_str()),
        ("C.UTF-8", "C.UTF-8")
    );
    assert!(!missing.contains_key("LC_ALL"), "LC_ALL 未设置时不新增");
    let darwin = terminal_env(&env(&[("LANG", "posix")]), true);
    assert_eq!(darwin["LANG"], "en_US.UTF-8");
    let c_all = terminal_env(&env(&[("LC_ALL", " c "), ("LANG", "zh_CN.UTF-8")]), false);
    assert_eq!(c_all["LC_ALL"], "zh_CN.UTF-8", "回退值取首个 UTF-8 locale");
    assert_eq!(
        c_all["LANG"], "zh_CN.UTF-8",
        "已配置的 UTF-8 locale 保持原样"
    );
    let configured = terminal_env(
        &env(&[("LANG", "en_GB.utf8"), ("LC_CTYPE", "de_DE.ISO-8859-1")]),
        false,
    );
    assert_eq!(configured["LANG"], "en_GB.utf8");
    assert_eq!(
        configured["LC_CTYPE"], "de_DE.ISO-8859-1",
        "非 C 的非 UTF-8 locale 不改写"
    );
}

// 修复：darwin 分支用平台 PATH 分隔符（TS path.delimiter 同），只在 macOS 运行；Windows 上模拟 darwin
// 会得到 `;` 分隔，无意义，限非 Windows。
#[cfg(not(windows))]
#[test]
fn darwin_path_keeps_user_order_then_appends_fallbacks_without_duplicates() {
    let result = terminal_env(&env(&[("PATH", "/usr/bin: /custom/bin :/usr/bin")]), true);
    assert_eq!(
        result["PATH"],
        "/usr/bin:/custom/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/local/sbin:/bin:/usr/sbin:/sbin"
    );
    assert_eq!(
        terminal_env(&env(&[("PATH", "/x")]), false)["PATH"],
        "/x",
        "非 macOS 不改 PATH"
    );
}

// 修复：回退候选是 /bin/zsh、/bin/sh 等 Unix shell，Windows 上无此语义，限 Unix。
#[cfg(unix)]
#[tokio::test]
async fn shell_prefers_executable_shell_var_then_falls_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let good = dir.path().join("fake-shell");
    tokio::fs::write(&good, "#!/bin/sh\n").await.expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let good = good.to_string_lossy().into_owned();
    assert_eq!(
        terminal_shell(&env(&[("SHELL", &good)]), false)
            .await
            .expect("shell"),
        good
    );
    let fallback = terminal_shell(&env(&[("SHELL", "/definitely/not/here")]), false)
        .await
        .expect("fallback");
    assert!(["/bin/zsh", "/bin/bash", "/bin/sh"].contains(&fallback.as_str()));
    // 修复：执行位只在 Unix 有意义；整段放进 cfg(unix)，否则 Windows 上 `plain` 未使用，-D warnings 报错。
    #[cfg(unix)]
    {
        let plain = dir.path().join("plain");
        tokio::fs::write(&plain, "x").await.expect("write");
        let plain = plain.to_string_lossy().into_owned();
        assert_ne!(
            terminal_shell(&env(&[("SHELL", &plain)]), false)
                .await
                .expect("shell"),
            plain,
            "无执行位的文件不能当 shell"
        );
    }
}

#[tokio::test]
async fn cwd_falls_back_to_home_then_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_string_lossy().into_owned();
    let vars = env(&[("HOME", &home)]);
    assert_eq!(terminal_cwd(&vars, Some(&home)).await.expect("cwd"), home);
    assert_eq!(
        terminal_cwd(&vars, Some("/gone/dir")).await.expect("cwd"),
        home
    );
    let file = dir.path().join("file");
    tokio::fs::write(&file, "x").await.expect("write");
    assert_eq!(
        terminal_cwd(&vars, Some(&file.to_string_lossy()))
            .await
            .expect("cwd"),
        home,
        "文件不是可用目录"
    );
    assert_eq!(terminal_cwd(&env(&[]), None).await.expect("cwd"), "/");
}
