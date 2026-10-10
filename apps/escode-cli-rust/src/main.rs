mod args;
mod headless;
mod runtime;
use anyhow::Result;
use args::Args;
use clap::Parser;
use escode_cli_app_server as stdio;
use escode_cli_state::Store;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    // `app-server` 走协议入口；其余参数按 Node 全局参数处理（`-p` 无头模式，docs/specs/rust-headless-prompt.md）。
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) != Some("app-server") {
        let code = headless::run(argv).await;
        std::process::exit(code);
    }
    if let Err(error) = run().await {
        // stderr 断管也不能递归进入错误处理；stdout 永远只用于协议。file_log 的 error 同时写 stderr。
        escode_cli_host::file_log::error(
            "runtime.failed",
            "cli",
            &error.to_string(),
            serde_json::Value::Null,
        );
        std::process::exit(1);
    }
    escode_cli_host::file_log::info(
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
    escode_cli_host::file_log::info(
        "runtime.started",
        "cli",
        "Runtime started",
        json!({"version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS, "arch": std::env::consts::ARCH, "prepareStorage": args.prepare_storage}),
    );
    if !args.prepare_storage {
        escode_cli_host::file_log::schedule_retention();
    }
    let ws = runtime::resolve_workspace(args.cwd.clone(), args.data_dir.clone()).await?;
    let cancel = CancellationToken::new();
    watch_signals(cancel.clone());
    let input_closed = CancellationToken::new();
    let (mut input, output, writer) = stdio::start(cancel.clone(), input_closed.clone());
    let attempt = escode_cli_host::id();
    let database_id = format!("{:x}", Sha256::digest(ws.db.to_string_lossy().as_bytes()));
    let progress = |phase: &str, sequence: u64| json!({"method":"startup/storageState","params":{"schemaVersion":1,"attemptId":attempt,"sequence":sequence,"databaseId":database_id,"databaseKind":"session","phase":phase,"elapsedMs":0}});
    if args.prepare_storage {
        stdio::storage_prepare(&ws.db, &mut input, &output).await?;
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
            let _ = stdio::finish(writer).await;
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
            stdio::finish(writer).await?;
            return Ok(());
        }
        if let Err(error) = imported {
            let mut frame = progress("failed", 2);
            frame["params"]["errorCode"] = "sql_failed".into();
            output.send(vec![frame]).await?;
            drop(output);
            let _ = stdio::finish(writer).await;
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
            runtime::build_engine(&ws, store, args.config.as_ref(), args.surface == "desktop")
                .await?
                .serve(input, output.clone(), cancel)
                .await
        }
        .await
    };
    // 修复：服务以错误结束（如 fault.storage.commit）时也先排空 writer 再退出。之前 `?` 直接返回，
    // 跳过 writer 收尾，main 随即 process::exit(1)，已入队的错误应答偶发丢失（macOS CI 复现）。
    drop(output);
    let finished = stdio::finish(writer).await;
    served?;
    finished?;
    Ok(())
}
