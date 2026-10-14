mod args;
mod headless;
mod wiki_cli;
use anyhow::Result;
use args::Args;
use clap::Parser;
use escode_cli_app_server::{Link, StdioLink};
use escode_cli_host::file_log;
use escode_cli_rust::runtime;
use escode_cli_state::Store;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

fn main() {
    // 进程级 env 快照（rust-u1-runtime-lib.md §3.1）：此后 crates/ 内的环境读取
    // 都走 core-api 访问器并以本快照兜底；嵌入实例则用各自的 RuntimeOptions。
    escode_cli_core_api::install_process_snapshot();
    // Windows 主线程默认只有 1MB 栈：工作流 TaskOutput 等深层异步调用链在 debug 构建下会以
    // 0xC00000FD 栈溢出（workflow-task-output 差分用例实测，docs/specs/rust-unified-host.md 阶段 0）。
    // 把整个 runtime 放到显式大栈线程上运行；join 失败按通用故障码退出。
    let entry = std::thread::Builder::new()
        .name("escode-runtime".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build the tokio runtime")
                .block_on(runtime_main())
        })
        .expect("Failed to spawn the runtime thread");
    if entry.join().is_err() {
        std::process::exit(1);
    }
}

async fn runtime_main() {
    // `app-server` 走协议入口；其余参数按 Node 全局参数处理（`-p` 无头模式，docs/specs/rust-headless-prompt.md）。
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // `wiki`：LLM Wiki 运维子命令（apps/escode-cli-rust/docs/llm-wiki/03-usage.md）。
    if argv.first().map(String::as_str) == Some("wiki") {
        std::process::exit(wiki_cli::run(argv[1..].to_vec()).await);
    }
    if argv.first().map(String::as_str) != Some("app-server") {
        let code = headless::run(argv).await;
        std::process::exit(code);
    }
    if let Err(error) = run().await {
        // stderr 断管也不能递归进入错误处理；stdout 永远只用于协议。file_log 的 error 同时写 stderr。
        file_log::error(
            "runtime.failed",
            "cli",
            &error.to_string(),
            serde_json::Value::Null,
        );
        // 所有权冲突用专用退出码，Host 据此等待旧 owner 退出后重试，而不是判定 Rust 不可用。
        if error.is::<escode_cli_state::WorkspaceOwned>() {
            std::process::exit(escode_cli_state::WORKSPACE_OWNED_EXIT_CODE);
        }
        std::process::exit(1);
    }
    file_log::info(
        "runtime.stopped",
        "cli",
        "Runtime stopped",
        serde_json::Value::Null,
    );
}

/// 进程收到 Ctrl-C / SIGTERM 时取消；返回收到的信号对应的退出码（130 / 143）。
pub(crate) fn watch_signals(cancel: CancellationToken) -> tokio::task::JoinHandle<i32> {
    tokio::spawn(async move {
        #[cfg(unix)]
        let code = {
            if let Ok(mut term) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {_=tokio::signal::ctrl_c()=>130,_=term.recv()=>143}
            } else {
                let _ = tokio::signal::ctrl_c().await;
                130
            }
        };
        #[cfg(not(unix))]
        let code = {
            let _ = tokio::signal::ctrl_c().await;
            130
        };
        cancel.cancel();
        code
    })
}

async fn run() -> Result<()> {
    let args = Args::parse();
    // 文件日志（docs/specs/rust-file-log.md）：进程生命周期 + 7 天保留。
    file_log::info(
        "runtime.started",
        "cli",
        "Runtime started",
        json!({"version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS, "arch": std::env::consts::ARCH, "prepareStorage": args.prepare_storage}),
    );
    if !args.prepare_storage {
        file_log::schedule_retention();
    }
    let ws = runtime::resolve_workspace(
        args.cwd.clone(),
        args.data_dir.clone(),
        escode_cli_core_api::env::var("ESCODE_WORKSPACE_IDENTITY").ok(),
    )
    .await?;
    let cancel = CancellationToken::new();
    watch_signals(cancel.clone());
    let input_closed = CancellationToken::new();
    let streams = Box::new(StdioLink).start(cancel.clone(), input_closed.clone());
    let escode_cli_app_server::Streams {
        mut input,
        output,
        writer,
    } = streams;
    let attempt = escode_cli_host::id();
    let database_id = format!("{:x}", Sha256::digest(ws.db.to_string_lossy().as_bytes()));
    let progress = |phase: &str, sequence: u64| json!({"method":"startup/storageState","params":{"schemaVersion":1,"attemptId":attempt,"sequence":sequence,"databaseId":database_id,"databaseKind":"session","phase":phase,"elapsedMs":0}});
    if args.prepare_storage {
        escode_cli_app_server::storage_prepare(&ws.db, &mut input, &output).await?;
    }
    let _owner = if args.prepare_storage {
        None
    } else {
        Some(Store::lock_workspace(ws.data_dir.clone(), ws.identity.clone()).await?)
    };
    output.send(vec![progress("checking", 1)]).await?;
    let store = match Store::open(ws.db.clone()).await {
        Ok(store) => store,
        Err(_) => {
            let mut frame = progress("failed", 2);
            frame["params"]["errorCode"] = "sql_failed".into();
            output.send(vec![frame]).await?;
            drop(output);
            let _ = escode_cli_app_server::finish(writer).await;
            anyhow::bail!("Session storage failed");
        }
    };
    // 模型层的用量事实（每次逻辑请求一条）落会话库（TS usage store）；记录器只持弱引用。
    escode_cli_core_api::set_model_usage_sink(store.usage_recorder());
    if !args.prepare_storage {
        // 导入期间 Host 关闭输入或进程收到信号：取消导入并直接退出。
        let stop = cancel.child_token();
        let (closed, stop_on_close) = (input_closed.clone(), stop.clone());
        tokio::spawn(async move {
            closed.cancelled().await;
            stop_on_close.cancel();
        });
        let imported = runtime::import_history(
            &store,
            &ws,
            args.import_ts_db.clone(),
            args.config.is_none(),
            &stop,
        )
        .await;
        if matches!(imported, Ok(true)) {
            drop(output);
            escode_cli_app_server::finish(writer).await?;
            return Ok(());
        }
        if let Err(error) = imported {
            let mut frame = progress("failed", 2);
            frame["params"]["errorCode"] = "sql_failed".into();
            output.send(vec![frame]).await?;
            drop(output);
            let _ = escode_cli_app_server::finish(writer).await;
            anyhow::bail!("TS history import failed; source remains unchanged: {error:#}");
        }
    }
    output.send(vec![progress("ready", 2)]).await?;
    let served: Result<()> = if args.prepare_storage {
        drop(store);
        output
            .send(vec![
                json!({"method":"startup/storagePrepared","params":{}}),
            ])
            .await
            .map_err(Into::into)
    } else {
        async {
            // 协议入口服务桌面/远控等 UI 客户端，有内联视图宿主：保留官方 visualize 技能
            // （Node 协议入口 includeVisualize=true）。
            runtime::build_engine(
                &ws,
                store,
                args.config.as_ref(),
                args.surface == "desktop",
                true,
            )
            .await?
            .serve(input, output.clone(), cancel)
            .await
        }
        .await
    };
    // 修复：服务以错误结束（如 fault.storage.commit）时也先排空 writer 再退出。之前 `?` 直接返回，
    // 跳过 writer 收尾，main 随即 process::exit(1)，已入队的错误应答偶发丢失（macOS CI 复现）。
    drop(output);
    let finished = escode_cli_app_server::finish(writer).await;
    served?;
    finished?;
    Ok(())
}
