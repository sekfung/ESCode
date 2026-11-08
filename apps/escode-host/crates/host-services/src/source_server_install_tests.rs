// 安装断言以本机 `sh` 执行真实 POSIX 脚本；Windows 主链由桌面 E2E 覆盖（S4-i）。
#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::process::Child;

use super::{InstallMode, build_install_plan, install_plan_locked};
use crate::source_server_archive::{ArchiveEntry, create_archive};
use crate::source_server_deploy_lock::AcquireOptions;
use crate::source_server_exec::{CommandOutput, RemoteRunner, run_command, spawn_piped_command};
use crate::source_server_preflight::{DownloadTool, RemoteAssetTools, Sha256Tool};
use crate::source_server_release::{ReleaseManifest, parse_component_meta, parse_release_manifest};
use crate::source_server_transport::TransportCommand;

struct LocalRunner {
    home: PathBuf,
}

impl LocalRunner {
    fn command(&self, script: &str) -> TransportCommand {
        TransportCommand {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: BTreeMap::from([("HOME".to_owned(), self.home.to_string_lossy().into_owned())]),
        }
    }
}

#[async_trait]
impl RemoteRunner for LocalRunner {
    async fn run(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> anyhow::Result<CommandOutput> {
        run_command(&self.command(script), stdin, timeout).await
    }

    fn spawn_piped(&self, script: &str) -> anyhow::Result<Child> {
        spawn_piped_command(&self.command(script))
    }
}

fn sha(value: u8) -> String {
    format!("{value:02x}").repeat(32)
}

fn manifest_json() -> Value {
    let components = [
        ("server-bundle", "server", "v3.15.2+11634cd500db"),
        ("node-runtime", "node/linux-x64", "v22.16.0+09fafeec368a"),
        (
            "node-pty",
            "node-pty/linux-x64",
            "v1.2.0-beta.10+a0067f3e5772",
        ),
        ("glm", "glm/linux-x64", "v0.13.3+4686f4d638ab"),
        ("bfs", "tools/linux-x64/bfs", "v4.1.1-2+ef9928797c6f"),
        (
            "ripgrep",
            "tools/linux-x64/ripgrep",
            "v14.1.1-1+1dd7516d37f6",
        ),
        ("ugrep", "tools/linux-x64/ugrep", "v7.8.4-1+89bdb72311dc"),
    ];
    let components: Vec<Value> = components
        .iter()
        .enumerate()
        .map(|(index, (id, mount, version))| {
            json!({
                "id": id,
                "version": version,
                "sha256": sha(index as u8 + 1),
                "artifactPath": format!("components/linux-x64/{id}/{version}.tar.gz"),
                "mount": mount,
            })
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "appVersion": "3.15.2",
        "platformArch": "linux-x64",
        "components": components,
    })
}

fn manifest() -> ReleaseManifest {
    parse_release_manifest(&manifest_json(), "3.15.2", "linux-x64").expect("manifest")
}

fn write(path: &Path, content: &str, executable: bool) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, content).expect("file");
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

fn release_dir(root: &Path) {
    write(&root.join("server/zcode-server.cjs"), "server", false);
    write(&root.join("node/linux-x64/node"), "node", true);
    write(&root.join("node-pty/linux-x64/pty.node"), "pty", false);
    write(&root.join("glm/linux-x64/escode-cli-rust"), "rust", true);
    write(&root.join("glm/linux-x64/plugin-host.cjs"), "plugin-host", false);
    write(&root.join("glm/linux-x64/packages/a.js"), "plugin", false);
    write(&root.join("tools/linux-x64/bfs/bfs"), "bfs", true);
    write(&root.join("tools/linux-x64/ripgrep/rg"), "rg", true);
    write(&root.join("tools/linux-x64/ugrep/ugrep"), "ugrep", true);
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[tokio::test]
async fn local_upload_installs_components_generated_files_and_meta() {
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let release = temp.path().join("release");
    release_dir(&release);
    let plan = build_install_plan(&manifest()).expect("plan");
    assert_eq!(plan.components.len(), 7);
    assert!(!plan.components[0].entries.is_empty());
    let lock = AcquireOptions {
        acquire_timeout: Duration::from_secs(10),
        ..AcquireOptions::default()
    };
    install_plan_locked(
        &LocalRunner { home: home.clone() },
        &plan,
        &InstallMode::LocalUpload {
            release_dir: release,
        },
        &lock,
    )
    .await
    .expect("install");
    let server = home.join(".escode/server");
    assert_eq!(read(&server.join("node")), "node");
    assert_eq!(read(&server.join("zcode-server.cjs")), "server");
    assert_eq!(read(&server.join("build/Release/pty.node")), "pty");
    assert_eq!(read(&server.join("agents/glm/escode-cli-rust")), "rust");
    assert_eq!(read(&server.join("agents/glm/plugin-host.cjs")), "plugin-host");
    assert_eq!(read(&server.join("agents/glm/packages/a.js")), "plugin");
    assert!(read(&server.join("agents/glm/escode-agent")).contains("/escode-cli-rust"));
    assert_eq!(read(&server.join("agents/glm/.version")), "0.13.3\n");
    assert_eq!(read(&server.join("tools/bfs/bfs")), "bfs");
    assert_eq!(read(&server.join("tools/ripgrep/rg")), "rg");
    assert_eq!(read(&server.join("tools/ugrep/ugrep")), "ugrep");
    assert_eq!(read(&server.join("tools/bfs/.version")), "4.1.1-2\n");

    let meta = parse_component_meta(&read(&server.join(".asset-components/node-runtime.json")))
        .expect("node meta");
    assert_eq!(meta.id, "node-runtime");
    assert_eq!(meta.platform_arch, "linux-x64");
    assert_eq!(meta.version.as_deref(), Some("v22.16.0+09fafeec368a"));
    assert!(meta.sha256.is_some());
    assert!(!server.join(".deploy.lock").exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(server.join("node"))
            .expect("node")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "node must be executable");
    }
}

fn artifacts(plan: &super::InstallPlan, release: &Path) -> BTreeMap<String, Vec<u8>> {
    plan.components
        .iter()
        .map(|component| {
            // 发布的 CDN 制品以 component mount 为归档根；fixture 必须保持同一布局，
            // 否则 remote-download 的 mount 相对路径会多出一层目录。
            let mount = component.component.mount.trim_end_matches('/');
            let entries: Vec<ArchiveEntry> = component
                .entries
                .iter()
                .flat_map(|entry| archive_entries(release, mount, &entry.source_relative_path))
                .collect();
            (
                format!("/{}", component.component.artifact_path),
                create_archive(&entries).expect("archive"),
            )
        })
        .collect()
}

/// 真实 CDN 制品把 component mount 的内容放在归档根：mount 是目录入口时按顶层子项展开，
/// 其余 entry 去掉 mount 前缀。
fn archive_entries(release: &Path, mount: &str, source_relative_path: &str) -> Vec<ArchiveEntry> {
    let source = release.join(source_relative_path);
    if source_relative_path.trim_matches('/') == mount {
        let mut children: Vec<std::fs::DirEntry> = std::fs::read_dir(&source)
            .expect("read mount directory")
            .collect::<Result<_, _>>()
            .expect("mount entries");
        children.sort_by_key(std::fs::DirEntry::file_name);
        return children
            .into_iter()
            .map(|child| ArchiveEntry {
                source_path: child.path(),
                archive_path: child.file_name().to_string_lossy().into_owned(),
            })
            .collect();
    }
    vec![ArchiveEntry {
        source_path: source,
        archive_path: source_relative_path
            .strip_prefix(&format!("{mount}/"))
            .unwrap_or(source_relative_path)
            .to_owned(),
    }]
}

fn serve(map: BTreeMap<String, Vec<u8>>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let address = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming().take(64) {
            let Ok(mut stream) = stream else { break };
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            match map.get(path) {
                Some(body) => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        }
    });
    format!("http://{address}")
}

fn has_command(name: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .status()
        .is_ok_and(|status| status.success())
}

#[tokio::test]
async fn remote_download_installs_from_the_manifest_artifact() {
    if !has_command("curl") || !has_command("sha256sum") {
        eprintln!("skipping remote-download test: curl/sha256sum unavailable");
        return;
    }
    let temp = tempfile::tempdir().expect("temp");
    let home = temp.path().join("home");
    let release = temp.path().join("release");
    release_dir(&release);
    let mut plan = build_install_plan(&manifest()).expect("plan");
    let mut served = artifacts(&plan, &release);
    let mut value = manifest_json();
    for (index, component) in value["components"]
        .as_array_mut()
        .expect("components")
        .iter_mut()
        .enumerate()
    {
        let artifact_path = component["artifactPath"].as_str().expect("artifact path");
        let body = served
            .get(&format!("/{artifact_path}"))
            .expect("served artifact");
        use sha2::Digest;
        component["sha256"] = json!(format!("{:x}", sha2::Sha256::digest(body)));
        let _ = index;
    }
    let manifest = parse_release_manifest(&value, "3.15.2", "linux-x64").expect("manifest");
    plan = build_install_plan(&manifest).expect("plan");
    served.clear();
    served.extend(artifacts(&plan, &release));
    let base_url = serve(served);
    let lock = AcquireOptions {
        acquire_timeout: Duration::from_secs(10),
        ..AcquireOptions::default()
    };
    install_plan_locked(
        &LocalRunner { home: home.clone() },
        &plan,
        &InstallMode::RemoteDownload {
            base_urls: vec![base_url],
            tools: RemoteAssetTools {
                download: DownloadTool::Curl,
                tar: true,
                sha256: Sha256Tool::Sha256sum,
            },
        },
        &lock,
    )
    .await
    .expect("install");

    let server = home.join(".escode/server");
    assert_eq!(read(&server.join("node")), "node");
    assert_eq!(read(&server.join("agents/glm/escode-cli-rust")), "rust");
    assert_eq!(read(&server.join("tools/ripgrep/rg")), "rg");
}

#[test]
fn plan_requires_node_and_server_components() {
    let mut value = manifest_json();
    value["components"]
        .as_array_mut()
        .expect("components")
        .retain(|component| component["id"] != json!("node-runtime"));
    let manifest = parse_release_manifest(&value, "3.15.2", "linux-x64").expect("manifest");
    let error = build_install_plan(&manifest).expect_err("missing required components");
    assert!(error.to_string().contains("node-runtime"), "{error}");
}
