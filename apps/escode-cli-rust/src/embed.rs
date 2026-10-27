//! 嵌入入口（docs/specs/rust-u1-runtime-lib.md §3.4；形态对标 ZCode-rs
//! `apps/zcode-cli-rust/src/embed.rs`）。
//!
//! 在调用方进程内启动一个运行时实例：嵌入方拿到内存链路的一端，协议与
//! `app-server --stdio` 完全一致——请求行进、应答与事件帧出；关闭写半即
//! stdin EOF，实例按与进程相同的语义收口。实例的每个任务都属于实例作用域
//! （`tasks.running()` 残余清单；实例任务结束时宽限收口并中止残余）。
//!
//! 与二进制入口的差异：不监听信号、不 `process::exit`（信号与退出码是进程
//! 概念，归调用方）；`WorkspaceOwned` 以错误类型返回（`downcast_ref` 判断）；
//! `--prepare-storage` 仍是二进制入口专有；TS 历史导入由 `RuntimeOptions.legacy_import`
//! 显式控制（默认不导入，Host 首启传 `Implicit`，见 `docs/specs/rust-u2-legacy-import.md`）。
use anyhow::Result;
use escode_cli_app_server::{EmbeddedIo, EmbeddedTypedIo, Link, MemoryLink, TypedMemoryLink};
use escode_cli_core_api::{EnvSnapshot, InstanceContext, Spawner, env};
use escode_cli_state::Store;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// 嵌入实例的 TS 历史导入策略（`docs/specs/rust-u2-legacy-import.md`）。
///
/// 默认 `Disabled`：测试/诊断嵌入方不会被进程环境影响；App Host 首启显式传 `Implicit`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LegacyImport {
    /// 不导入。
    #[default]
    Disabled,
    /// 按 `legacy_paths::resolve(None, cwd, true)` 自动发现。
    Implicit,
    /// 显式来源；不存在即失败。
    Explicit(PathBuf),
}

/// 一个嵌入实例的装配参数。`env` 是实例可见的完整环境（缺失的 HOME /
/// USERPROFILE 以 `home` 补齐）；`argv` 与 `std::env::args` 同形（含 argv[0]）；
/// `cwd` 是 Host 提交的 workspace 路径（不 realpath）；`workspace_identity`
/// 缺省按路径回退（AGENTS.md Workspace Identity 规则）；`config` 是静态模型
/// 配置（与 CLI `--config` 同格式；Host 装配时通常改由 Registry 环境文件供给）。
pub struct RuntimeOptions {
    pub home: PathBuf,
    pub env: BTreeMap<String, String>,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub workspace_identity: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub legacy_import: LegacyImport,
}

/// 实例收口宽限：serve 返回后等待任务排空的上限，超时中止残余并在错误里点名。
const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

pub struct Embedded {
    /// 嵌入方一端：写入请求行（NDJSON），读出应答与事件帧。
    pub io: EmbeddedIo,
    /// 实例任务：运行时停止时完成；错误即原因（含 `WorkspaceOwned`）。
    pub task: JoinHandle<Result<()>>,
    /// 请求实例排空并停止（与进程收到 SIGTERM 等价）。
    pub cancel: CancellationToken,
    /// 实例任务作用域：`running()` 残余清单（关闭日志与测试断言用）。
    pub tasks: Arc<Spawner>,
}

/// 启动一个运行时实例（在当前 tokio runtime 上）。
pub fn spawn(options: RuntimeOptions) -> Embedded {
    let snapshot = instance_snapshot(&options);
    let spawner = Spawner::new();
    let context = Arc::new(InstanceContext {
        env: Arc::new(snapshot),
        spawner: Arc::downgrade(&spawner),
        usage_sink: OnceLock::new(),
        media_cache_root: OnceLock::new(),
    });
    spawner.bind_context(context.clone());
    let cancel = CancellationToken::new();
    let input_closed = CancellationToken::new();
    let (link, io) = MemoryLink::channel(64);
    let streams = Box::new(link).start(cancel.clone(), input_closed.clone());
    let (task, tasks) = spawn_instance(
        options,
        streams,
        context,
        cancel.clone(),
        input_closed,
        spawner,
    );
    Embedded {
        io,
        task,
        cancel,
        tasks,
    }
}

/// 类型化直传的嵌入实例（docs/specs/rust-u4-memory-link.md）：帧面是 `serde_json::Value`，
/// 进程内不做 NDJSON 编解码。Host 的 embedded 链路使用本入口；字节版 [`spawn`] 保留给
/// 差分与故障注入。
pub struct EmbeddedTyped {
    /// 嵌入方一端：发送/接收 JSON 帧（不再有行与换行）。
    pub io: EmbeddedTypedIo,
    pub task: JoinHandle<Result<()>>,
    pub cancel: CancellationToken,
    pub tasks: Arc<Spawner>,
}

/// 启动一个类型化运行时实例（与 [`spawn`] 同一套装配与收口语义）。
pub fn spawn_typed(options: RuntimeOptions) -> EmbeddedTyped {
    let snapshot = instance_snapshot(&options);
    let spawner = Spawner::new();
    let context = Arc::new(InstanceContext {
        env: Arc::new(snapshot),
        spawner: Arc::downgrade(&spawner),
        usage_sink: OnceLock::new(),
        media_cache_root: OnceLock::new(),
    });
    spawner.bind_context(context.clone());
    let cancel = CancellationToken::new();
    let input_closed = CancellationToken::new();
    let (link, io) = TypedMemoryLink::channel(64);
    let streams = Box::new(link).start(cancel.clone(), input_closed.clone());
    let (task, tasks) = spawn_instance(
        options,
        streams,
        context,
        cancel.clone(),
        input_closed,
        spawner,
    );
    EmbeddedTyped {
        io,
        task,
        cancel,
        tasks,
    }
}

/// 实例装配的共用部分（字节链路与类型化链路只差 `Streams` 的来源）。
fn spawn_instance(
    options: RuntimeOptions,
    streams: escode_cli_app_server::Streams,
    context: Arc<InstanceContext>,
    cancel: CancellationToken,
    input_closed: CancellationToken,
    tasks: Arc<Spawner>,
) -> (JoinHandle<Result<()>>, Arc<Spawner>) {
    let spawner = Arc::clone(&tasks);
    let task = tokio::spawn(async move {
        // 实例根未来整体跑在实例上下文里：装配与 serve 的每个环境/端口读取
        // 都以 RuntimeOptions 快照解析（与进程 env/cwd 无关）。
        // 修复：根任务 panic（engine 循环 catch_panic 之外的装配/收尾路径）原先直接展开整个任务，
        // 跳过下面的 abort_all，残余任务持有 Output 克隆让 Host 读端迟迟不关、在飞请求挂到超时。
        // 捕获后转为错误值，残余照常中止（docs/specs/rust-u2-host-resilience.md §2.7）。
        let served = escode_cli_core_api::catch_panic(env::run_in_instance(
            context,
            // 先装箱：实例根 future 很大（debug 构建尤甚），多套一层 catch_panic 会在 worker 栈上
            // 多留一份临时副本，2 MiB 的 tokio worker 栈随即溢出（host-rpc rpc_agent_v4 实测）。
            Box::pin(run_embedded(options, streams, cancel, input_closed)),
        ))
        .await
        .unwrap_or_else(|panicked| {
            Err(anyhow::anyhow!(
                "embedded runtime panicked: {}",
                panicked.message
            ))
        });
        // 与 ZCode-rs scope 语义一致：实例任务结束后中止仍在飞的残余任务。
        tasks.abort_all();
        served
    });
    (task, spawner)
}

/// RuntimeOptions → 实例环境快照（HOME/USERPROFILE 缺失时以 `home` 补齐）。
fn instance_snapshot(options: &RuntimeOptions) -> EnvSnapshot {
    let mut vars: Vec<(OsString, OsString)> = options
        .env
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect();
    let home = options.home.to_string_lossy().into_owned();
    for key in ["HOME", "USERPROFILE"] {
        let present = vars.iter().any(|(name, _)| name == key);
        if !present {
            vars.push((OsString::from(key), OsString::from(&home)));
        }
    }
    let mut argv = options.argv.clone();
    if argv.is_empty() {
        argv.push("escode-embedded".to_owned());
    }
    let exe = std::env::current_exe().unwrap_or_default();
    EnvSnapshot::from_parts(vars, options.cwd.clone(), argv, exe)
}

/// 实例装配与运行：与二进制 `run()` 同一套顺序（锁 → 存储 → 引擎 → serve →
/// 排空写出），只在传输（内存链路）与入口语义（无信号/无退出码）上不同。
async fn run_embedded(
    options: RuntimeOptions,
    streams: escode_cli_app_server::Streams,
    cancel: CancellationToken,
    _input_closed: CancellationToken,
) -> Result<()> {
    let escode_cli_app_server::Streams {
        input,
        output,
        writer,
    } = streams;
    let config = options.config.clone();
    let ws = crate::runtime::resolve_workspace(
        Some(options.cwd.clone()),
        options.data_dir.clone(),
        options.workspace_identity.clone(),
    )
    .await?;
    let database_id = format!("{:x}", Sha256::digest(ws.db.to_string_lossy().as_bytes()));
    let progress = |phase: &str, sequence: u64| json!({"method":"startup/storageState","params":{"schemaVersion":1,"attemptId":escode_cli_host::id(),"sequence":sequence,"databaseId":database_id,"databaseKind":"session","phase":phase,"elapsedMs":0}});
    let _owner = Store::lock_workspace(ws.data_dir.clone(), ws.identity.clone()).await?;
    let store = Store::open(ws.db.clone()).await?;
    // 模型层用量事实落本实例的会话库：实例上下文里的 set 是实例槽，多实例互不串扰。
    escode_cli_core_api::set_model_usage_sink(store.usage_recorder());
    output.send(vec![progress("checking", 1)]).await?;
    // H3.5：策略由嵌入方给出；导入语义与 CLI `import_history` 完全同源。
    let (explicit, allow_implicit) = match options.legacy_import {
        LegacyImport::Disabled => (None, false),
        LegacyImport::Implicit => (None, true),
        LegacyImport::Explicit(path) => (Some(path), false),
    };
    if explicit.is_some() || allow_implicit {
        match crate::runtime::import_history(&store, &ws, explicit, allow_implicit, &cancel).await {
            Ok(true) => {
                // 取消：与 CLI 相同，不发出 ready/failed。
                drop(output);
                let _ = escode_cli_app_server::finish(writer).await;
                return Ok(());
            }
            Ok(false) => {}
            Err(error) => {
                let mut frame = progress("failed", 2);
                frame["params"]["errorCode"] = json!("sql_failed");
                let _ = output.send(vec![frame]).await;
                drop(output);
                let _ = escode_cli_app_server::finish(writer).await;
                return Err(error.context("TS history import failed; source remains unchanged"));
            }
        }
    }
    output.send(vec![progress("ready", 2)]).await?;
    // 协议入口（嵌入方为 Host 形态）：desktop 呈现面 + 内联视图宿主，与
    // `app-server --stdio --surface desktop` 相同（Node includeVisualize=true）。
    let served = crate::runtime::build_engine(&ws, store, config.as_ref(), true, true)
        .await?
        .serve(input, output.clone(), cancel)
        .await;
    // 与二进制相同：服务结束（含错误路径）先排空已入队输出再汇总，错误应答不丢。
    drop(output);
    let finished = escode_cli_app_server::finish(writer).await;
    // 实例收口：宽限等待任务排空；残余清单告警（abort_all 由根任务在返回前执行）。
    let residual = Spawner::current().drain(DRAIN_GRACE).await;
    if !residual.is_empty() {
        escode_cli_host::file_log::warn(
            "runtime.residual-tasks",
            "cli",
            &format!("residual tasks at shutdown: {}", residual.join(", ")),
            serde_json::Value::Null,
        );
    }
    served?;
    finished?;
    Ok(())
}
