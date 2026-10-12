//! `escode-cli-rust wiki …`：LLM Wiki 的运维入口（apps/escode-cli-rust/docs/llm-wiki/03-usage.md）。
//! 与 runtime 使用同一个 WikiService（同一存储、同一项目身份）。批准、弃用、提升作用域是高影响操作：
//! 交互终端里逐次确认，非交互环境必须显式 `--yes`。
use clap::{Parser, Subcommand};
use escode_cli_domain::wiki::entry::Scope;
use escode_cli_domain::wiki::tools::{ManageInput, ProposeInput};
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "escode-cli-rust wiki",
    about = "ESCode LLM Wiki: ingest, search, review and maintain knowledge"
)]
struct WikiArgs {
    /// 工作区（决定 Project Wiki）；缺省为当前目录。
    #[arg(long, global = true)]
    cwd: Option<PathBuf>,
    /// 输出 JSON。
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 项目身份、存储位置、条目与来源统计。
    Status,
    /// 导入文件或目录（默认导入为候选，--approve 表示用户直接批准）。
    Ingest {
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "project")]
        scope: String,
        #[arg(long)]
        approve: bool,
        #[arg(long)]
        yes: bool,
    },
    /// 检查已知来源的修改、移动与删除。
    Sync {
        #[arg(long)]
        scope: Option<String>,
    },
    Search {
        query: Vec<String>,
        #[arg(long)]
        include_proposals: bool,
        #[arg(long)]
        workflow: Option<String>,
    },
    Read {
        id: String,
    },
    /// 新建候选知识（不会自动成为规范）。
    Propose {
        #[arg(long)]
        title: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        #[arg(long)]
        rationale: String,
        #[arg(long, default_value = "project")]
        scope: String,
    },
    Approve {
        ids: Vec<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    Reject {
        ids: Vec<String>,
        #[arg(long)]
        reason: Option<String>,
    },
    Deprecate {
        ids: Vec<String>,
        #[arg(long)]
        superseded_by: Option<String>,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// 把项目条目复制到 Global Wiki（作为新的候选）。
    Promote {
        ids: Vec<String>,
        #[arg(long)]
        yes: bool,
    },
    Lint,
    Validate,
    Conflicts,
    RebuildIndex,
    Audit {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// 知识图谱（节点与边），与 Desktop 图谱页同一数据。
    Graph,
}

fn confirm(question: &str, yes: bool) -> anyhow::Result<()> {
    if yes {
        return Ok(());
    }
    anyhow::ensure!(
        std::io::stdin().is_terminal(),
        "{question} — refusing without --yes in a non-interactive shell"
    );
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    anyhow::ensure!(matches!(answer.trim(), "y" | "Y" | "yes"), "cancelled");
    Ok(())
}

fn scope(value: &str) -> anyhow::Result<Scope> {
    Scope::parse(value).ok_or_else(|| anyhow::anyhow!("scope must be project or global"))
}

fn print(value: &Value, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    } else if let Some(summary) = value["summary"].as_str() {
        println!("{summary}");
        for item in value["items"].as_array().into_iter().flatten() {
            println!("  {}", serde_json::to_string(item).unwrap_or_default());
        }
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    }
}

fn manage(action: &str) -> ManageInput {
    ManageInput {
        action: action.into(),
        ..Default::default()
    }
}

pub async fn run(argv: Vec<String>) -> i32 {
    let args = match WikiArgs::try_parse_from(std::iter::once("wiki".to_owned()).chain(argv)) {
        Ok(args) => args,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() { 2 } else { 0 };
        }
    };
    match execute(args).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("wiki: {error:#}");
            1
        }
    }
}

async fn execute(args: WikiArgs) -> anyhow::Result<()> {
    let cwd = match args.cwd {
        Some(path) => std::path::absolute(path)?,
        None => std::env::current_dir()?,
    };
    let wiki = crate::runtime::wiki_service(&cwd);
    let json = args.json;
    let by = "cli";
    let result = match args.command {
        Command::Status => wiki.status().await?,
        Command::Ingest {
            paths,
            scope: s,
            approve,
            yes,
        } => {
            if approve {
                confirm(
                    &format!(
                        "Approve all knowledge imported from {} path(s) as {s} standards?",
                        paths.len()
                    ),
                    yes,
                )?;
            }
            let report = wiki.ingest(&paths, scope(&s)?, approve).await?;
            serde_json::json!({"summary": report.summary(), "items": report.items, "ok": report.ok()})
        }
        Command::Sync { scope: s } => {
            wiki.manage(
                &ManageInput {
                    scope: s,
                    ..manage("sync")
                },
                by,
            )
            .await?
        }
        Command::Search {
            query,
            include_proposals,
            workflow,
        } => {
            let input = serde_json::json!({"query": query.join(" "), "include_proposals": include_proposals, "workflow": workflow});
            let mut input = input;
            if input["workflow"].is_null() {
                input.as_object_mut().map(|o| o.remove("workflow"));
            }
            let out = wiki.tool("WikiSearch", &input, "cli").await?;
            if !json {
                println!("{}", out.content);
                return Ok(());
            }
            out.structured
        }
        Command::Read { id } => {
            let out = wiki
                .tool("WikiRead", &serde_json::json!({"id": id}), "cli")
                .await?;
            anyhow::ensure!(!out.failed, "{}", out.content);
            if !json {
                println!("{}", out.content);
                return Ok(());
            }
            out.structured
        }
        Command::Propose {
            title,
            kind,
            body,
            body_file,
            rationale,
            scope: s,
        } => {
            let body = match (body, body_file) {
                (Some(b), _) => b,
                (None, Some(path)) => tokio::fs::read_to_string(path).await?,
                (None, None) => anyhow::bail!("--body or --body-file is required"),
            };
            let input = ProposeInput {
                title,
                body,
                kind,
                rationale,
                scope: Some(s),
                tags: None,
                applies_to_paths: None,
                evidence: None,
            };
            let (entry, duplicates) = wiki.propose(&input, "cli").await?;
            serde_json::json!({"id": entry.meta.id, "scope": entry.meta.scope.as_str(), "status": "proposal", "duplicate_of": duplicates})
        }
        Command::Approve { ids, reason, yes } => {
            confirm(
                &format!(
                    "Approve {} entr(ies) as standards: {}?",
                    ids.len(),
                    ids.join(", ")
                ),
                yes,
            )?;
            wiki.manage(
                &ManageInput {
                    ids: Some(ids),
                    reason,
                    ..manage("approve")
                },
                by,
            )
            .await?
        }
        Command::Reject { ids, reason } => {
            wiki.manage(
                &ManageInput {
                    ids: Some(ids),
                    reason,
                    ..manage("reject")
                },
                by,
            )
            .await?
        }
        Command::Deprecate {
            ids,
            superseded_by,
            reason,
            yes,
        } => {
            confirm(&format!("Deprecate {}?", ids.join(", ")), yes)?;
            wiki.manage(
                &ManageInput {
                    ids: Some(ids),
                    reason,
                    superseded_by,
                    ..manage("deprecate")
                },
                by,
            )
            .await?
        }
        Command::Promote { ids, yes } => {
            confirm(
                &format!(
                    "Copy {} to the global wiki (as proposals for every project)?",
                    ids.join(", ")
                ),
                yes,
            )?;
            wiki.manage(
                &ManageInput {
                    ids: Some(ids),
                    ..manage("promote")
                },
                by,
            )
            .await?
        }
        Command::Lint => wiki.manage(&manage("lint"), by).await?,
        Command::Validate => wiki.manage(&manage("validate"), by).await?,
        Command::Conflicts => wiki.manage(&manage("conflicts"), by).await?,
        Command::RebuildIndex => wiki.manage(&manage("rebuild_index"), by).await?,
        Command::Audit { limit } => {
            serde_json::json!({"summary": "audit", "items": wiki.audit_tail(limit).await?})
        }
        Command::Graph => wiki.graph(&serde_json::json!({"limit": 2000})).await?,
    };
    let failed = result["ok"] == false;
    print(&result, json);
    anyhow::ensure!(!failed, "one or more operations failed");
    Ok(())
}
