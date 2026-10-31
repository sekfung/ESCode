//! 真实 SSH 首装（S4-c 退出条件）：空目标 → preflight → local-upload 安装 → 启动 server → Initialize。
//!
//! 未设置 `ESCODE_E2E_SSH_HOST` 时跳过。需要 release 目录（默认仓库
//! `packages/desktop/mock-cdn/releases/<ESCODE_E2E_SSH_APP_VERSION>`）。设置
//! `ESCODE_E2E_SSH_FIRST_INSTALL=1` 时要求本次确实执行了首次安装。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use escode_host_services::source_server_bootstrap::{
    RemoteAssetsConfig, apply_install, prepare_install,
};
use escode_host_services::source_server_connection::{ConnectOptions, connect};
use escode_host_services::source_server_deploy_lock::{AcquireOptions, acquire};
use escode_host_services::source_server_exec::RemoteExecutor;
use escode_host_services::source_server_exec::RemoteRunner;
use escode_host_services::source_server_install::InstallMode;
use escode_host_services::source_server_transport::{SshTarget, TransportSecrets, TransportTarget};

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Windows 上 Rust Host 带 `.exe` 后缀；默认 askpass 必须指向真实可执行文件（S4-i）。
fn default_askpass_path() -> PathBuf {
    let name = if cfg!(windows) {
        "escode-host.exe"
    } else {
        "escode-host"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(name)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

#[tokio::test(flavor = "multi_thread")]
async fn installs_release_over_real_ssh_and_starts_server() {
    let Some(host) = env("ESCODE_E2E_SSH_HOST") else {
        eprintln!("skipping real SSH install: ESCODE_E2E_SSH_HOST is not set");
        return;
    };
    let username = env("ESCODE_E2E_SSH_USERNAME").expect("ESCODE_E2E_SSH_USERNAME");
    let port: u16 = env("ESCODE_E2E_SSH_PORT")
        .map(|value| value.parse().expect("ESCODE_E2E_SSH_PORT"))
        .unwrap_or(22);
    let password = env("ESCODE_E2E_SSH_PASSWORD");
    let askpass = env("ESCODE_E2E_SSH_ASKPASS")
        .map(PathBuf::from)
        .unwrap_or_else(default_askpass_path);
    let app_version = env("ESCODE_E2E_SSH_APP_VERSION").unwrap_or_else(|| "3.15.2".to_owned());
    let release_dir = env("ESCODE_E2E_SSH_RELEASE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_root()
                .join("packages/desktop/mock-cdn/releases")
                .join(&app_version)
        });
    let expect_first_install = env("ESCODE_E2E_SSH_FIRST_INSTALL").is_some();

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
    let executor = RemoteExecutor::new(target, secrets, askpass)
        .with_known_hosts(Some(known_hosts.path().join("known_hosts")));
    let assets = RemoteAssetsConfig {
        release_dir: Some(release_dir),
        cdn_base_urls: Vec::new(),
        app_version: app_version.clone(),
    };

    let prepare_started = Instant::now();
    if let Some(prepared) = prepare_install(&executor, &assets)
        .await
        .expect("prepare install")
    {
        let preview = prepared.preview.as_ref().expect("local-upload preview");
        assert!(preview.size_bytes > 0);
        eprintln!(
            "first install planned: {} {} bytes in {}ms",
            prepared.plan.platform_arch,
            preview.size_bytes,
            prepare_started.elapsed().as_millis()
        );
        let apply_started = Instant::now();
        apply_install(
            &executor,
            &prepared,
            &AcquireOptions {
                acquire_timeout: Duration::from_secs(30),
                release_timeout: Duration::from_secs(10),
                ..AcquireOptions::default()
            },
        )
        .await
        .expect("install over ssh");
        eprintln!(
            "first install applied in {}ms",
            apply_started.elapsed().as_millis()
        );
    } else {
        assert!(
            !expect_first_install,
            "ESCODE_E2E_SSH_FIRST_INSTALL was requested but the target was already installed"
        );
        eprintln!("target already had the release; continuing with server start");
    }
    assert!(
        prepare_install(&executor, &assets)
            .await
            .expect("re-inspect after install")
            .is_none(),
        "installed components must be idempotent"
    );

    let script = "ESCODE_SERVICE_AUTHORITY_MODE=\"desktop-attached-remote\" ESCODE_SERVER_RUNTIME_ROOT=\"$HOME/.escode/server\" ~/.escode/server/node ~/.escode/server/zcode-server.cjs";
    let command = executor.build_command(script).expect("server command");
    let connection = connect(
        &command,
        &ConnectOptions {
            client_id: "escode-host-install-e2e",
            client_version: &app_version,
            handshake_timeout: Duration::from_secs(15),
        },
    )
    .await
    .expect("hello");
    tokio::time::timeout(
        Duration::from_secs(20),
        connection.client.wait_initialized(),
    )
    .await
    .expect("initialize timeout")
    .expect("initialize");
    assert_eq!(connection.hello.version, app_version);
    connection.shutdown().await.expect("shutdown");
}

/// 远端下载模式（spec §11.2）：本地只有 manifest，artifact 由远端 curl 从本地 HTTP 服务下载。
///
/// 只在显式设置 `ESCODE_E2E_SSH_REMOTE_DOWNLOAD=1` 时运行：它会和 local-upload 用例一样
/// 要求空目标，并行跑两组安装会互相覆盖。
#[tokio::test(flavor = "multi_thread")]
async fn installs_release_by_remote_download_over_real_ssh() {
    if env("ESCODE_E2E_SSH_REMOTE_DOWNLOAD").is_none() {
        eprintln!(
            "skipping real SSH remote-download install: ESCODE_E2E_SSH_REMOTE_DOWNLOAD is not set"
        );
        return;
    }
    let Some(host) = env("ESCODE_E2E_SSH_HOST") else {
        eprintln!("skipping real SSH remote-download install: ESCODE_E2E_SSH_HOST is not set");
        return;
    };
    let username = env("ESCODE_E2E_SSH_USERNAME").expect("ESCODE_E2E_SSH_USERNAME");
    let port: u16 = env("ESCODE_E2E_SSH_PORT")
        .map(|value| value.parse().expect("ESCODE_E2E_SSH_PORT"))
        .unwrap_or(22);
    let password = env("ESCODE_E2E_SSH_PASSWORD");
    let askpass = env("ESCODE_E2E_SSH_ASKPASS")
        .map(PathBuf::from)
        .unwrap_or_else(default_askpass_path);
    let app_version = env("ESCODE_E2E_SSH_APP_VERSION").unwrap_or_else(|| "3.15.2".to_owned());
    let mock_cdn = workspace_root().join("packages/desktop/mock-cdn");
    // release 目录只放 manifest：本地没有解包产物，choose_mode 必须选择 remote-download。
    let release = tempfile::tempdir().expect("temp release");
    std::fs::copy(
        mock_cdn
            .join("releases")
            .join(&app_version)
            .join("manifest-linux-x64.json"),
        release.path().join("manifest-linux-x64.json"),
    )
    .expect("copy release manifest");
    let base_url = serve_static(mock_cdn);
    let known_hosts = tempfile::tempdir().expect("known_hosts dir");
    let executor = RemoteExecutor::new(
        TransportTarget::Ssh(SshTarget {
            host,
            port,
            username,
            private_key_path: None,
            password_credential_key: password.as_ref().map(|_| "e2e-password".to_owned()),
            private_key_passphrase_credential_key: None,
        }),
        TransportSecrets {
            password,
            private_key_passphrase: None,
        },
        askpass,
    )
    .with_known_hosts(Some(known_hosts.path().join("known_hosts")));
    let assets = RemoteAssetsConfig {
        release_dir: Some(release.path().to_path_buf()),
        cdn_base_urls: vec![base_url],
        app_version,
    };
    let prepared = prepare_install(&executor, &assets)
        .await
        .expect("prepare remote-download install")
        .expect("empty target must report missing components");
    assert!(
        matches!(prepared.mode, InstallMode::RemoteDownload { .. }),
        "expected remote-download mode"
    );
    apply_install(
        &executor,
        &prepared,
        &AcquireOptions {
            acquire_timeout: Duration::from_secs(30),
            release_timeout: Duration::from_secs(10),
            ..AcquireOptions::default()
        },
    )
    .await
    .expect("install by remote download over ssh");
    assert!(
        prepare_install(&executor, &assets)
            .await
            .expect("re-inspect after remote download")
            .is_none(),
        "installed components must be idempotent"
    );
    let installed = executor
        .run(
            "test -x ~/.escode/server/node && test -f ~/.escode/server/zcode-server.cjs && echo installed",
            None,
            Duration::from_secs(15),
        )
        .await
        .expect("verify installed files");
    assert!(installed.stdout.contains("installed"));
}

/// 以 mock-cdn 根目录为站点根，供容器内 curl 拉取 `components/<platform>/...`。
fn serve_static(root: PathBuf) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind static server");
    let address = listener.local_addr().expect("static server address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            match std::fs::read(root.join(path.trim_start_matches('/'))) {
                Ok(body) => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                }
                Err(_) => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        }
    });
    format!("http://{address}")
}

/// 单独验证部署锁在真实 SSH 上的 release 语义（安装脚本本身不参与）。
#[tokio::test(flavor = "multi_thread")]
async fn deploy_lock_releases_over_real_ssh() {
    let Some(host) = env("ESCODE_E2E_SSH_HOST") else {
        eprintln!("skipping real SSH deploy lock: ESCODE_E2E_SSH_HOST is not set");
        return;
    };
    let username = env("ESCODE_E2E_SSH_USERNAME").expect("ESCODE_E2E_SSH_USERNAME");
    let port: u16 = env("ESCODE_E2E_SSH_PORT")
        .map(|value| value.parse().expect("ESCODE_E2E_SSH_PORT"))
        .unwrap_or(22);
    let password = env("ESCODE_E2E_SSH_PASSWORD");
    let askpass = env("ESCODE_E2E_SSH_ASKPASS")
        .map(PathBuf::from)
        .unwrap_or_else(default_askpass_path);
    let target = TransportTarget::Ssh(SshTarget {
        host,
        port,
        username,
        private_key_path: None,
        password_credential_key: password.as_ref().map(|_| "e2e-password".to_owned()),
        private_key_passphrase_credential_key: None,
    });
    let known_hosts = tempfile::tempdir().expect("known_hosts dir");
    let executor = RemoteExecutor::new(
        target,
        TransportSecrets {
            password,
            private_key_passphrase: None,
        },
        askpass,
    )
    .with_known_hosts(Some(known_hosts.path().join("known_hosts")));
    let lock_dir = format!(
        "~/.escode/e2e-lock-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis()
    );
    let options = AcquireOptions {
        lock_dir,
        acquire_timeout: Duration::from_secs(30),
        release_timeout: Duration::from_secs(30),
        ..AcquireOptions::default()
    };
    let lock = acquire(&executor, &options)
        .await
        .expect("acquire over ssh");
    lock.release().await.expect("release over ssh");
}
