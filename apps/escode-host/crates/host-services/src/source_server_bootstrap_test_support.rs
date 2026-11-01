//! S4-c 测试共用的 release/runner fixture（本地 `sh`，不访问网络）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::bail;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::process::Child;

#[cfg(unix)]
use super::*;
// 仅 `connections_with_identity`（`#[cfg(unix)]`）使用；非 unix 目标导入会触发 `-D warnings`，
// 与 2026-10-31 S4-i 修复的同类 Windows clippy 失败一致。
#[cfg(unix)]
use crate::source_server::ServerIdentity;
#[cfg(unix)]
use crate::source_server_connections::{ConnectionsOptions, SourceServerConnections};
use crate::source_server_exec::{CommandOutput, RemoteRunner};
#[cfg(unix)]
use crate::source_server_exec::{RemoteLink, ResolvedTransport, run_command, spawn_piped_command};
use crate::source_server_transport::TransportCommand;

pub(super) fn platform_arch() -> &'static str {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "darwin-arm64"
        } else {
            "darwin-x64"
        }
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-x64"
    }
}

fn sha(value: u8) -> String {
    format!("{value:02x}").repeat(32)
}

pub(super) fn manifest_json(platform: &str) -> Value {
    let components: [(&str, String, &str); 7] = [
        ("server-bundle", "server".to_owned(), "v3.15.2+11634cd500db"),
        (
            "node-runtime",
            format!("node/{platform}"),
            "v22.16.0+09fafeec368a",
        ),
        (
            "node-pty",
            format!("node-pty/{platform}"),
            "v1.2.0-beta.10+a0067f3e5772",
        ),
        ("glm", format!("glm/{platform}"), "v0.13.3+4686f4d638ab"),
        (
            "bfs",
            format!("tools/{platform}/bfs"),
            "v4.1.1-2+ef9928797c6f",
        ),
        (
            "ripgrep",
            format!("tools/{platform}/ripgrep"),
            "v14.1.1-1+1dd7516d37f6",
        ),
        (
            "ugrep",
            format!("tools/{platform}/ugrep"),
            "v7.8.4-1+89bdb72311dc",
        ),
    ];
    let components: Vec<Value> = components
        .iter()
        .enumerate()
        .map(|(index, (id, mount, version))| {
            json!({
                "id": id,
                "version": version,
                "sha256": sha(index as u8 + 1),
                "artifactPath": format!("components/{platform}/{id}/{version}.tar.gz"),
                "mount": mount,
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "appVersion": "3.15.2",
        "platformArch": platform,
        "components": components,
    })
}

#[cfg_attr(not(unix), allow(unused_variables))]
pub(super) fn write(path: &Path, content: &str, executable: bool) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, content).expect("file");
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

pub(super) fn write_manifest(release_dir: &Path, platform: &str) {
    write(
        &release_dir.join(format!("manifest-{platform}.json")),
        &manifest_json(platform).to_string(),
        false,
    );
}

pub(super) fn write_release(release_dir: &Path, platform: &str) {
    write_manifest(release_dir, platform);
    write(
        &release_dir.join("server/zcode-server.cjs"),
        "server",
        false,
    );
    write(
        &release_dir.join(format!("node/{platform}/node")),
        "node",
        true,
    );
    write(
        &release_dir.join(format!("node-pty/{platform}/pty.node")),
        "pty",
        false,
    );
    // `component_install` 在 `darwin-*` 上还要求 `spawn-helper`；夹具缺它会让 macOS 的
    // local-upload 用例全部落到「local release artifacts are incomplete」。
    if platform.starts_with("darwin-") {
        write(
            &release_dir.join(format!("node-pty/{platform}/spawn-helper")),
            "spawn-helper",
            true,
        );
    }
    write(
        &release_dir.join(format!("glm/{platform}/zcode.cjs")),
        "glm",
        false,
    );
    write(
        &release_dir.join(format!("tools/{platform}/bfs/bfs")),
        "bfs",
        true,
    );
    write(
        &release_dir.join(format!("tools/{platform}/ripgrep/rg")),
        "rg",
        true,
    );
    write(
        &release_dir.join(format!("tools/{platform}/ugrep/ugrep")),
        "ugrep",
        true,
    );
}

pub(super) fn probe_stdout(tools: bool) -> String {
    let (download, sha) = if tools {
        ("curl", "sha256sum")
    } else {
        ("", "")
    };
    let platform = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    // 修复：`kernel_ostype` 取 `/proc/sys/kernel/ostype`，macOS 上不存在（真实探测输出为空）。
    // 之前恒发 `Linux`，`resolve_remote_platform` 会把 Darwin+Linux 判成 `linux-*`
    // （WSL 语义），macOS 用例于是去找 `manifest-linux-arm64.json` 而全部失败。
    let kernel = if cfg!(target_os = "macos") {
        ""
    } else {
        "Linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    format!(
        "platform={platform}\narch={arch}\nkernel_ostype={kernel}\nhome=/home/dev\ndownload={download}\ntar=tar\nsha256={sha}\n"
    )
}

/// 本地 `sh` runner：脚本在临时 HOME 上真实执行（探测、锁、安装共用一条路径）。
#[cfg(unix)]
pub(super) struct LocalRunner {
    home: PathBuf,
    calls: AtomicUsize,
}

#[cfg(unix)]
impl LocalRunner {
    pub(super) fn new(home: PathBuf) -> Self {
        Self {
            home,
            calls: AtomicUsize::new(0),
        }
    }

    pub(super) fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    fn command(&self, script: &str) -> TransportCommand {
        TransportCommand {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: BTreeMap::from([("HOME".to_owned(), self.home.to_string_lossy().into_owned())]),
        }
    }
}

#[cfg(unix)]
#[async_trait]
impl RemoteRunner for LocalRunner {
    async fn run(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> anyhow::Result<CommandOutput> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        run_command(&self.command(script), stdin, timeout).await
    }

    fn spawn_piped(&self, script: &str) -> anyhow::Result<Child> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        spawn_piped_command(&self.command(script))
    }
}

pub(super) struct FakeRunner {
    pub(super) stdout: String,
}

#[async_trait]
impl RemoteRunner for FakeRunner {
    async fn run(
        &self,
        _script: &str,
        _stdin: Option<&[u8]>,
        _timeout: Duration,
    ) -> anyhow::Result<CommandOutput> {
        Ok(CommandOutput {
            stdout: self.stdout.clone(),
            stderr: String::new(),
            exit_code: Some(0),
        })
    }

    fn spawn_piped(&self, _script: &str) -> anyhow::Result<Child> {
        bail!("fake runner has no piped execution")
    }
}

pub(super) const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
pub(super) const PROFILE: &str = "44444444-4444-4444-8444-444444444444";

pub(super) fn registry() -> Arc<crate::source_server::SourceServerRegistry> {
    registry_with_declared(&[])
}

pub(super) fn registry_with_declared(
    workspaces: &[(&str, &str)],
) -> Arc<crate::source_server::SourceServerRegistry> {
    let declared: Vec<Value> = workspaces
        .iter()
        .map(|(path, identity)| {
            json!({
                "workspacePath": path,
                "workspaceIdentity": identity,
            })
        })
        .collect();
    Arc::new(
        crate::source_server::SourceServerRegistry::from_settings(&json!({
            "sourceServers": [{
                "sourceServerId": SOURCE,
                "displayName": "fixture",
                "declaredWorkspaces": declared,
            }],
            "connectionProfiles": [{
                "profileId": PROFILE,
                "sourceServerId": SOURCE,
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "fixture", "username": "dev"},
                "createdAt": 1_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .expect("registry"),
    )
}

pub(super) fn fixture_command() -> TransportCommand {
    fixture_command_with_env(&[])
}

/// 测试 fixture 的环境变体：版本、任务状态或缺失 `escode-task` channel。
pub(super) fn fixture_command_with_env(extra: &[(&str, &str)]) -> TransportCommand {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let mut env = BTreeMap::from([("ESCODE_FIXTURE_HELLO".to_owned(), "1".to_owned())]);
    for (key, value) in extra {
        env.insert((*key).to_owned(), (*value).to_owned());
    }
    TransportCommand {
        program: PathBuf::from("node"),
        args: vec![
            "--import".into(),
            "tsx".into(),
            root.join("apps/escode-host/tests/fixtures/rpc_server_fixture.mjs")
                .into_os_string(),
        ],
        env,
    }
}

#[cfg(unix)]
pub(super) fn connections_with_identity(
    registry: Arc<crate::source_server::SourceServerRegistry>,
    runner: Arc<LocalRunner>,
    identity: ServerIdentity,
    credentials: Option<Arc<crate::credential::CredentialService>>,
) -> SourceServerConnections {
    let runner_for_resolver = Arc::clone(&runner);
    let options = ConnectionsOptions::new("3.15.2", move |_| {
        let runner = Arc::clone(&runner_for_resolver);
        Box::pin(async move {
            Ok(ResolvedTransport {
                link: RemoteLink::Process(fixture_command()),
                runner,
            })
        })
    })
    .with_identity_probe(Some(Arc::new(move |_runner| {
        let identity = identity.clone();
        Box::pin(async move { Ok(identity) })
    })))
    .with_credentials(credentials);
    SourceServerConnections::new(registry, options)
}

#[cfg(unix)]
pub(super) fn connections(
    registry: Arc<crate::source_server::SourceServerRegistry>,
    runner: Arc<LocalRunner>,
    assets: Option<RemoteAssetsConfig>,
) -> SourceServerConnections {
    let runner_for_resolver = Arc::clone(&runner);
    let options = ConnectionsOptions::new("3.15.2", move |_| {
        let runner = Arc::clone(&runner_for_resolver);
        Box::pin(async move {
            Ok(ResolvedTransport {
                link: RemoteLink::Process(fixture_command()),
                runner,
            })
        })
    })
    .with_install(assets.map(|assets| InstallOptions {
        assets,
        lock: AcquireOptions {
            acquire_timeout: Duration::from_secs(10),
            release_timeout: Duration::from_secs(5),
            ..AcquireOptions::default()
        },
    }));
    SourceServerConnections::new(registry, options)
}
