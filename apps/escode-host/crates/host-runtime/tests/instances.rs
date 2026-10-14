//! 实例表与 runtime-link（rust-unified-host.md §2 生命周期；u2 子 spec §3/§5 第 9 项）：
//! 每 workspace 一实例、懒重启 generation、single-flight、收口残余清单、embedded/child 双链路同帧面。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use escode_cli_rust::embed::RuntimeOptions;
use escode_host_runtime::{Instances, Link};
use serde_json::Value;

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    data_dir: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temp dir");
        let workspace = root.path().join("workspace");
        let data_dir = root.path().join("data");
        let home = root.path().join("home");
        for dir in [&workspace, &data_dir, &home] {
            std::fs::create_dir_all(dir).expect("fixture dir");
        }
        Self {
            _root: root,
            workspace,
            data_dir,
            home,
        }
    }

    fn identity(&self) -> String {
        self.workspace.to_string_lossy().into_owned()
    }

    fn options(&self) -> RuntimeOptions {
        let mut env = BTreeMap::new();
        env.insert("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned());
        RuntimeOptions {
            home: self.home.clone(),
            env,
            argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
            cwd: self.workspace.clone(),
            workspace_identity: Some(self.identity()),
            data_dir: Some(self.data_dir.clone()),
            config: None,
        }
    }
}

/// 读帧直到 startup/storageState 的 ready 帧（checking 先到）。
async fn wait_ready(instance: &escode_host_runtime::Instance) {
    loop {
        let line = tokio::time::timeout(Duration::from_secs(30), instance.line())
            .await
            .expect("frame within timeout")
            .expect("instance output open");
        let value: Value = serde_json::from_str(&line).expect("NDJSON frame");
        if value["method"] == "startup/storageState" && value["params"]["phase"] == "ready" {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_workspaces_get_independent_instances() {
    let first = Fixture::new();
    let second = Fixture::new();
    let instances = Instances::new(Link::Embedded);

    let a = instances
        .get(&first.identity(), || first.options())
        .await
        .expect("instance A starts");
    let b = instances
        .get(&second.identity(), || second.options())
        .await
        .expect("instance B starts");
    wait_ready(&a).await;
    wait_ready(&b).await;
    assert!(a.is_alive() && b.is_alive());
    assert_eq!(
        instances.generations(),
        BTreeMap::from([(first.identity(), 1), (second.identity(), 1)]),
        "每个 workspace 各自第一代"
    );
    // 第二次 get 命中同一个实例（不新起代次）。
    let again = instances
        .get(&first.identity(), || first.options())
        .await
        .expect("reuse");
    assert!(std::sync::Arc::ptr_eq(&a, &again), "活着就复用");
    assert_eq!(instances.generations()[&first.identity()], 1);

    let residual = instances.shutdown().await;
    assert!(residual.is_empty(), "收口无残余: {residual:?}");
    assert!(instances.identities().len() == 2, "槽位保留世代记账");
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_triggers_lazy_restart_with_next_generation() {
    let fixture = Fixture::new();
    let instances = Instances::new(Link::Embedded);
    let first = instances
        .get(&fixture.identity(), || fixture.options())
        .await
        .expect("first generation");
    wait_ready(&first).await;
    // 收口当前实例（等价于崩溃/退出后 unavailable）。
    let _ = instances.close(&fixture.identity()).await;
    assert!(instances.live(&fixture.identity()).is_none());
    // 懒重启只在下次 get 发生，且代次 +1。
    let second = instances
        .get(&fixture.identity(), || fixture.options())
        .await
        .expect("second generation");
    wait_ready(&second).await;
    assert_eq!(second.generation, first.generation + 1);
    assert_eq!(instances.generations()[&fixture.identity()], 2);
    let _ = instances.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_gets_start_exactly_one_instance() {
    let fixture = Fixture::new();
    let instances = Instances::new(Link::Embedded);
    let identity = fixture.identity();
    let mut handles = Vec::new();
    for _ in 0..4 {
        let instances = instances.clone();
        let identity = identity.clone();
        let options = fixture.options();
        handles.push(tokio::spawn(async move {
            instances
                .get(&identity, || options)
                .await
                .expect("instance")
        }));
    }
    let mut ids = Vec::new();
    for handle in handles {
        ids.push(handle.await.expect("join"));
    }
    let first = ids.first().expect("at least one").clone();
    assert!(
        ids.iter()
            .all(|instance| std::sync::Arc::ptr_eq(instance, &first)),
        "single-flight：并发 get 只启动一个实例"
    );
    assert_eq!(instances.generations()[&fixture.identity()], 1);
    let _ = instances.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn close_reports_residuals_then_shutdown_is_clean() {
    let fixture = Fixture::new();
    let instances = Instances::new(Link::Embedded).with_grace(Duration::from_secs(5));
    let instance = instances
        .get(&fixture.identity(), || fixture.options())
        .await
        .expect("instance");
    wait_ready(&instance).await;
    let residual = instances.close(&fixture.identity()).await.expect("close");
    assert!(residual.is_empty(), "收口后残余任务为空: {residual:?}");
    assert!(instances.live(&fixture.identity()).is_none());
    assert!(
        instances.shutdown().await.is_empty(),
        "无实例时 shutdown 无残余"
    );
}

/// child 链路与 embedded 同帧面（Host 的差分/回退选项）：CLI 二进制在场时才跑。
#[tokio::test(flavor = "multi_thread")]
async fn child_link_serves_the_same_ready_frame() {
    let Some(binary) = cli_binary() else {
        eprintln!("skip: escode-cli-rust 二进制不在场（先构建 CLI workspace）");
        return;
    };
    let fixture = Fixture::new();
    let instances = Instances::new(Link::Child { binary });
    let instance = instances
        .get(&fixture.identity(), || fixture.options())
        .await
        .expect("child instance starts");
    wait_ready(&instance).await;
    assert!(instance.is_alive());
    let residual = instances.shutdown().await;
    assert!(residual.is_empty(), "child 收口无残余: {residual:?}");
}

fn cli_binary() -> Option<PathBuf> {
    let candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../escode-cli-rust/target/debug/escode-cli-rust");
    candidate.is_file().then_some(candidate)
}
