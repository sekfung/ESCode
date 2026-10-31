//! 真实 SSH preflight（S4-b 退出条件）：对 Docker SSH fixture 执行一次组合探测。
//!
//! 未设置 `ESCODE_E2E_SSH_HOST` 时跳过，保持常规 `cargo test` 无外部依赖。
//! 需要 `ESCODE_E2E_SSH_USERNAME`；密码路径用 `ESCODE_E2E_SSH_PASSWORD`，
//! 并通过 `ESCODE_E2E_SSH_ASKPASS`（默认 `target/debug/escode-host`）提供 OpenSSH askpass。

use std::path::PathBuf;

use escode_host_services::source_server_preflight::{PREFLIGHT_TIMEOUT, probe_over_transport};
use escode_host_services::source_server_transport::{SshTarget, TransportSecrets, TransportTarget};

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[tokio::test]
async fn probes_the_real_ssh_fixture() {
    let Some(host) = env("ESCODE_E2E_SSH_HOST") else {
        eprintln!("skipping real SSH preflight: ESCODE_E2E_SSH_HOST is not set");
        return;
    };
    let username = env("ESCODE_E2E_SSH_USERNAME").expect("ESCODE_E2E_SSH_USERNAME");
    let port = env("ESCODE_E2E_SSH_PORT")
        .map(|value| value.parse().expect("ESCODE_E2E_SSH_PORT"))
        .unwrap_or(22);
    let expected_platform_arch =
        env("ESCODE_E2E_SSH_PLATFORM_ARCH").unwrap_or_else(|| "linux-x64".to_owned());
    let password = env("ESCODE_E2E_SSH_PASSWORD");
    let askpass = env("ESCODE_E2E_SSH_ASKPASS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let name = if cfg!(windows) {
                "escode-host.exe"
            } else {
                "escode-host"
            };
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug")
                .join(name)
        });

    let target = TransportTarget::Ssh(SshTarget {
        host,
        port,
        username,
        private_key_path: None,
        password_credential_key: password.as_ref().map(|_| "e2e-password".to_owned()),
        private_key_passphrase_credential_key: None,
    });
    let secrets = TransportSecrets {
        password,
        private_key_passphrase: None,
    };
    let known_hosts = tempfile::tempdir().expect("known_hosts dir");
    let known_hosts_path = known_hosts.path().join("known_hosts");
    let probe = probe_over_transport(
        &target,
        &secrets,
        &askpass,
        Some(&known_hosts_path),
        PREFLIGHT_TIMEOUT,
    )
    .await
    .expect("remote preflight probe");

    assert_eq!(probe.environment.platform_arch, expected_platform_arch);
    let home = probe.environment.home.as_deref().expect("remote HOME");
    assert!(home.starts_with('/'), "remote HOME={home}");
    assert!(probe.tools.tar, "tar must be present in the SSH fixture");
    eprintln!(
        "preflight: {} download={:?} sha256={:?} home={home}",
        probe.environment.platform_arch, probe.tools.download, probe.tools.sha256
    );
}
