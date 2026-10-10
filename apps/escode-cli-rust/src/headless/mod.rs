//! `-p` 无头模式（docs/specs/rust-headless-prompt.md）：参数、装配与进程内驱动、输出。
mod args;
mod client;
mod driver;

use anyhow::{Context, Result};
use args::{OutputFormat, Parsed};
use escode_cli_state::Store;
use serde_json::{Value, json};
use std::io::Write;
use tokio_util::sync::CancellationToken;

const HELP: &str = "Usage:
  escode-cli-rust -p <prompt> [options]
  escode-cli-rust app-server --stdio [options]

Options:
  -p, --prompt <text>             Run one prompt headlessly and print the result
      --output-format <format>    text (default) or json
      --json                      Alias for --output-format json
      --mode <mode>               build, edit, plan or yolo
      --cwd <path>                Workspace directory
      --attach <path>             Attach a file (repeatable)
      --resume <sessionId>        Continue a session
  -c, --continue                  Continue the most recent session in the workspace
      --disallowed-tools <tools>  Tools to disallow for this run
      --surface <surface>         terminal (default) or desktop
      --locale <locale>           en-US, zh-CN or auto
      --verbose                   Print error causes
      --config <model.json>       Single model configuration
      --data-dir <path>           Rust runtime data directory
  -h, --help                      Show this help
  -v, --version                   Show the version
";
const NO_MODEL: &str = "No model configured: pass --config <model.json> or set ESCODE_BUILTIN_PROVIDER_CONFIG_FILE and ESCODE_PERSONAL_PROVIDER_CONFIG_FILE.";

/// 运行 `-p` 并返回进程退出码。
pub async fn run(argv: Vec<String>) -> i32 {
    let mut stderr = std::io::stderr();
    let parsed = match args::parse(argv) {
        Ok(parsed) => parsed,
        Err(error) => {
            // Node：parseArgs 类错误在提示后空一行打印帮助。
            let help = if error.usage {
                format!("\n{HELP}")
            } else {
                String::new()
            };
            let _ = write!(stderr, "{}\n{help}", error.message);
            return 1;
        }
    };
    if parsed.help {
        print!("{HELP}");
        return 0;
    }
    if parsed.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    let Some(prompt) = parsed.prompt.as_deref() else {
        let _ = writeln!(
            stderr,
            "The Rust runtime has no interactive TUI; use -p/--prompt."
        );
        return 2;
    };
    if prompt.trim().is_empty() {
        let _ = writeln!(stderr, "--prompt requires non-empty text.");
        return 1;
    }
    let cancel = CancellationToken::new();
    let signal = crate::watch_signals(cancel.clone());
    let result = execute(&parsed, cancel.clone()).await;
    if cancel.is_cancelled() && signal.is_finished() {
        // 信号退出（Node shutdown.ts）：取消已由 Engine 收口，按信号码退出。
        return signal.await.unwrap_or(130);
    }
    signal.abort();
    match result {
        Ok((outcome, code)) => {
            print!("{outcome}");
            code
        }
        Err((error, trace)) => {
            let trace = trace
                .map(|t| format!(" (traceId: {t})"))
                .unwrap_or_default();
            let _ = writeln!(stderr, "Error: {error}{trace}");
            if parsed.verbose
                && let Some(cause) = error.chain().nth(1)
            {
                let _ = writeln!(stderr, "Cause: {cause}");
            }
            1
        }
    }
}

type Failure = (anyhow::Error, Option<String>);

/// 装配 Engine（与 app-server 同一套），驱动一轮，按格式渲染 stdout 文本。
async fn execute(parsed: &Parsed, cancel: CancellationToken) -> Result<(String, i32), Failure> {
    let ws = crate::runtime::resolve_workspace(parsed.cwd.clone(), parsed.data_dir.clone())
        .await
        .map_err(|e| (e, None))?;
    if !crate::runtime::model_configured(parsed.config.as_ref()) {
        return Err((anyhow::anyhow!(NO_MODEL), None));
    }
    let opened = async {
        let owner = Store::lock_workspace(ws.data_dir.clone(), ws.identity.clone()).await?;
        let store = Store::open(ws.db.clone())
            .await
            .context("Session storage failed")?;
        escode_cli_core_api::set_model_usage_sink(store.usage_recorder());
        crate::runtime::import_history(&store, &ws, None, parsed.config.is_none(), &cancel).await?;
        let engine =
            crate::runtime::build_engine(&ws, store, parsed.config.as_ref(), parsed.desktop)
                .await?
                // Node `-p` 不生成会话标题（titleGenerationEnabled: false）。
                .without_title_generation()
                // 无审批面：需要审批的工具直接拒绝（Node headless deny broker）。
                .with_headless_permissions();
        anyhow::Ok((owner, engine))
    }
    .await
    .map_err(|e| (e, None))?;
    let (_owner, engine) = opened;
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(64);
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Vec<Value>>(64);
    let serving = tokio::spawn(engine.serve(in_rx, out_tx, cancel.clone()));
    let mut client = client::Client::new(in_tx, out_rx);
    let workspace = ws.requested_cwd.to_string_lossy().into_owned();
    let outcome = tokio::select! {
        outcome = driver::run_turn(&mut client, parsed, &workspace, &ws.cwd) => outcome,
        _ = cancel.cancelled() => Err(anyhow::anyhow!("CLI received signal")),
    };
    drop(client);
    let _ = serving.await;
    let outcome = outcome.map_err(|e| (e, None))?;
    if outcome.phase != "completedSuccess" {
        let message = outcome
            .last_error
            .clone()
            .unwrap_or_else(|| format!("Turn ended: {}", outcome.phase));
        return Err((anyhow::anyhow!(message), outcome.trace_id.clone()));
    }
    Ok((
        render(&outcome, parsed.output.unwrap_or(OutputFormat::Text)),
        0,
    ))
}

/// 按给定键序渲染的 JSON 值（工作区 serde_json 未开 preserve_order，Map 按字典序；Node `formatJson` 保留插入顺序）。
enum Ordered {
    Leaf(Value),
    Object(Vec<(&'static str, Ordered)>),
}

fn pretty(value: &Ordered, indent: usize) -> String {
    match value {
        Ordered::Leaf(v) => serde_json::to_string(v).unwrap_or_default(),
        Ordered::Object(pairs) if pairs.is_empty() => "{}".into(),
        Ordered::Object(pairs) => {
            let pad = "  ".repeat(indent + 1);
            let body: Vec<String> = pairs
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{pad}{}: {}",
                        serde_json::to_string(k).unwrap_or_default(),
                        pretty(v, indent + 1)
                    )
                })
                .collect();
            format!("{{\n{}\n{}}}", body.join(",\n"), "  ".repeat(indent))
        }
    }
}

const USAGE_KEYS: [&str; 10] = [
    "source",
    "modelRequestCount",
    "inputTokens",
    "outputTokens",
    "totalTokens",
    "cacheReadTokens",
    "cacheWriteTokens",
    "reasoningTokens",
    "webFetchRequests",
    "webSearchRequests",
];

/// text：`{response}` + 换行；json：Node `formatJson`（2 空格缩进 + 换行），键序见 spec。
fn render(o: &driver::Outcome, format: OutputFormat) -> String {
    use Ordered::{Leaf, Object};
    if format == OutputFormat::Text {
        return format!("{}\n", o.response);
    }
    let mut pairs = vec![
        ("sessionId", Leaf(o.session_id.clone().into())),
        ("traceId", Leaf(json!(o.trace_id))),
    ];
    if let Some(turn) = &o.turn_id {
        pairs.push(("turnId", Leaf(turn.clone().into())));
    }
    pairs.push(("response", Leaf(o.response.clone().into())));
    if let Some(usage) = &o.usage {
        pairs.push((
            "usage",
            Object(
                USAGE_KEYS
                    .iter()
                    .map(|k| (*k, Leaf(usage[*k].clone())))
                    .collect(),
            ),
        ));
    }
    pairs.push(("eventCount", Leaf(o.event_count.into())));
    let total = o
        .usage
        .as_ref()
        .map_or(Value::from(0), |u| u["totalTokens"].clone());
    pairs.push((
        "projection",
        Object(vec![
            ("status", Leaf("idle".into())),
            ("turnCount", Leaf(1.into())),
            ("totalTokenCount", Leaf(total)),
            ("contextUsed", Leaf(o.context_used.clone())),
            ("contextWindow", Leaf(o.context_window.clone())),
        ]),
    ));
    format!("{}\n", pretty(&Object(pairs), 0))
}
