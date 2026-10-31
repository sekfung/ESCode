//! `--ssh-askpass` 模式（S3 transport）：只输出 env 中的 secret，不启动 Host 控制通道。

use std::process::Command;

#[test]
fn askpass_mode_prints_the_secret() {
    let output = Command::new(env!("CARGO_BIN_EXE_escode-host"))
        .arg("--ssh-askpass")
        .env("ESCODE_SSH_ASKPASS_SECRET", "s3cr3t")
        .output()
        .expect("host binary runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "s3cr3t\n");
}

#[test]
fn askpass_mode_accepts_openssh_prompt_argument() {
    // OpenSSH 调用 SSH_ASKPASS 时不带 `--ssh-askpass`，只把提示语作为 argv[1] 传入。
    let output = Command::new(env!("CARGO_BIN_EXE_escode-host"))
        .arg("root@127.0.0.1's password:")
        .env("ESCODE_SSH_ASKPASS_SECRET", "s3cr3t")
        .output()
        .expect("host binary runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "s3cr3t\n");
}

#[test]
fn askpass_mode_fails_without_secret() {
    let output = Command::new(env!("CARGO_BIN_EXE_escode-host"))
        .arg("--ssh-askpass")
        .env_remove("ESCODE_SSH_ASKPASS_SECRET")
        .output()
        .expect("host binary runs");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("ssh askpass secret missing"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn askpass_mode_fails_with_openssh_prompt_and_no_secret() {
    let output = Command::new(env!("CARGO_BIN_EXE_escode-host"))
        .arg("root@127.0.0.1's password:")
        .env_remove("ESCODE_SSH_ASKPASS_SECRET")
        .output()
        .expect("host binary runs");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("ssh askpass secret missing"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
