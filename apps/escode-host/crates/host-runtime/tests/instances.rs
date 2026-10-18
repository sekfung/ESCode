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
    let mut frames = instance.subscribe().await;
    loop {
        let line = tokio::time::timeout(Duration::from_secs(30), frames.recv())
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

/// 实例已关闭的失败是 “closed”，不是超时（stale 实例恢复据此决定回收+重试）。
///
/// 修复：超时可分类的断言移到 host_io.rs（从不应答的子进程脚本，确定性超时）。这里原先用
/// 零期限 + 内嵌 runtime：多线程下应答可能在 send_line 期间就已入 channel，而 tokio `timeout`
/// 先轮询内部 future，零期限也会拿到应答（ubuntu 全量运行间歇失败）。
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_instance_fails_as_closed_not_as_timeout() {
    let fixture = Fixture::new();
    let instances = Instances::new(Link::Embedded);
    let instance = instances
        .get(&fixture.identity(), || fixture.options())
        .await
        .expect("instance starts");
    wait_ready(&instance).await;

    // 实例收口后的失败是 “closed”，不能被当成 stale 超时去重试。
    let _ = instances.shutdown().await;
    let closed = instance
        .call_with(
            "plugins/list",
            serde_json::json!({}),
            None,
            Duration::from_secs(5),
        )
        .await
        .expect_err("closed instance");
    assert!(
        closed
            .downcast_ref::<escode_host_runtime::RequestTimeout>()
            .is_none(),
        "closed 不是超时: {closed:#}"
    );
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

/// `live_any()`（不按 workspace 路由的服务的取用口，`rust-u2-misc-channels.md` §1.2）：
/// 空表回 None、活实例按**身份键字典序**取第一个、关闭后回 None（不留已退出实例）。
/// 字典序是登记过的语义（TS 是连接序），故在此钉住，改动即失败。
#[tokio::test(flavor = "multi_thread")]
async fn live_any_picks_the_lexicographically_first_identity() {
    let instances = Instances::new(Link::Embedded);
    assert!(instances.live_any().is_none(), "空表没有活实例");

    // 两次都起在同一个临时根下，只有目录名不同 —— 身份键的字典序即 `aaa` < `zzz`。
    let root = tempfile::tempdir().unwrap();
    let options_for = |name: &str| {
        let workspace = root.path().join(name);
        let home = root.path().join(format!("{name}-home"));
        let data_dir = root.path().join(format!("{name}-data"));
        for dir in [&workspace, &home, &data_dir] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let mut env = BTreeMap::new();
        env.insert("ESCODE_RUNTIME_ENV".to_owned(), "test".to_owned());
        let options = RuntimeOptions {
            home,
            env,
            argv: vec!["escode-cli-rust".to_owned(), "app-server".to_owned()],
            cwd: workspace.clone(),
            workspace_identity: Some(workspace.to_string_lossy().into_owned()),
            data_dir: Some(data_dir),
            config: None,
        };
        (workspace.to_string_lossy().into_owned(), options)
    };

    // 先起 `zzz`：若实现按插入序（先起的优先）而不是字典序，下面的断言会取到它。
    let (zzz, zzz_options) = options_for("zzz");
    let (aaa, aaa_options) = options_for("aaa");
    let late = instances.get(&zzz, || zzz_options).await.unwrap();
    let early = instances.get(&aaa, || aaa_options).await.unwrap();
    wait_ready(&late).await;
    wait_ready(&early).await;

    let picked = instances.live_any().expect("有两个活实例");
    assert!(
        std::sync::Arc::ptr_eq(&picked, &early),
        "按身份键字典序取 `aaa`，不是先起的 `zzz`"
    );

    instances.close(&aaa).await.unwrap();
    let picked = instances.live_any().expect("`zzz` 仍活着");
    assert!(
        std::sync::Arc::ptr_eq(&picked, &late),
        "已关闭的实例不再被选中"
    );
    instances.shutdown().await;
    assert!(instances.live_any().is_none(), "全部关闭后回到 None");
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
        .join("..")
        .join("..")
        .join("..")
        .join("escode-cli-rust")
        .join("target")
        .join("debug")
        .join("escode-cli-rust");
    candidate.is_file().then_some(candidate)
}
