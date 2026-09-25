//! 与 TS oracle 语料逐条比对（scripts/generate-zcode-cli-rust-memory-corpus.mjs）。
use super::memory::{self, Decision, DecisionMessage, ManifestEntry, ToolKind};
use serde_json::Value;

fn corpus() -> Value {
    serde_json::from_str(include_str!("../tests/fixtures/memory_corpus.json")).unwrap()
}

#[test]
fn index_and_section_match_ts() {
    let corpus = corpus();
    for case in corpus["indexes"].as_array().unwrap() {
        assert_eq!(
            memory::format_project_index(case["content"].as_str().unwrap()),
            case["formatted"].as_str().unwrap(),
            "{}",
            case["content"]
        );
    }
    assert_eq!(
        memory::section("/store/cli/memories/projects/repo-0123456789abcdef/memory"),
        corpus["section"].as_str().unwrap()
    );
}

fn manifest(corpus: &Value) -> Vec<ManifestEntry> {
    let mut entries: Vec<ManifestEntry> = corpus["manifestFiles"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(name, _)| name.ends_with(".md") && !name.ends_with("MEMORY.md"))
        .map(|(name, content)| {
            let preview = content
                .as_str()
                .unwrap()
                .split('\n')
                .take(memory::MANIFEST_PREVIEW_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            let (description, kind) = memory::manifest_frontmatter(&preview);
            ManifestEntry {
                description,
                filename: name.clone(),
                mtime_ms: corpus["mtimes"][name].as_f64().unwrap(),
                kind,
            }
        })
        .collect();
    entries.sort_by(|a, b| b.mtime_ms.total_cmp(&a.mtime_ms));
    entries
}

#[test]
fn manifest_and_prompt_match_ts() {
    let corpus = corpus();
    let entries = manifest(&corpus);
    let expected: Vec<Value> = corpus["manifest"].as_array().unwrap().clone();
    assert_eq!(entries.len(), expected.len());
    for (entry, expected) in entries.iter().zip(&expected) {
        assert_eq!(entry.filename, expected["filename"], "{expected}");
        assert_eq!(
            entry.description.as_deref(),
            expected["description"].as_str(),
            "{expected}"
        );
        assert_eq!(
            entry.kind.as_deref(),
            expected["type"].as_str(),
            "{expected}"
        );
    }
    for case in corpus["extractionPrompts"].as_array().unwrap() {
        let used = if case["manifest"].as_array().unwrap().is_empty() {
            vec![]
        } else {
            entries.clone()
        };
        assert_eq!(
            memory::format_manifest(&used),
            case["formattedManifest"].as_str().unwrap()
        );
        assert_eq!(
            memory::extraction_prompt(&used, case["messageCount"].as_u64().unwrap() as usize),
            case["prompt"].as_str().unwrap()
        );
    }
}

#[test]
fn tool_policy_matches_ts() {
    let corpus = corpus();
    let root = corpus["rootDir"].as_str().unwrap();
    let known = [
        "Read",
        "Grep",
        "Glob",
        "Write",
        "Edit",
        "Bash",
        "Agent",
        "mcp__srv__tool",
        "TodoWrite",
    ];
    for case in corpus["policies"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let kind = if name == "WebFetch" {
            ToolKind::Network
        } else if known.contains(&name) {
            ToolKind::Local
        } else {
            ToolKind::Missing
        };
        let result = memory::tool_policy(name, &case["input"], kind, root, "/work/repo", &|c| {
            crate::bash_policy::is_readonly(c)
        });
        match result {
            Ok(()) => assert_eq!(case["allowed"], true, "{case}"),
            Err(reason) => {
                assert_eq!(case["allowed"], false, "{case}");
                assert_eq!(reason, case["reason"].as_str().unwrap(), "{case}");
            }
        }
    }
}

#[test]
fn extraction_decision_matches_ts() {
    let corpus = corpus();
    let root = corpus["rootDir"].as_str().unwrap();
    for case in corpus["decisions"].as_array().unwrap() {
        let messages: Vec<DecisionMessage> = case["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                if let Some(text) = m["user"].as_str() {
                    DecisionMessage::User {
                        prose: m["synthetic"] != true
                            && m["modelOnly"] != true
                            && memory::is_prose(text),
                    }
                } else {
                    DecisionMessage::Assistant {
                        writes: m["write"].as_str().map(str::to_owned).into_iter().collect(),
                    }
                }
            })
            .collect();
        let cursor = case["cursorAfter"].as_u64().map(|c| c as usize);
        match memory::decide(&messages, cursor, root, "/work/repo") {
            Decision::Run(count) => {
                assert_eq!(case["run"], true, "{case}");
                assert_eq!(case["messageCount"], count, "{case}");
            }
            Decision::Skip => assert_eq!(case["run"], false, "{case}"),
        }
    }
}

#[test]
fn origin_stamp_matches_ts() {
    let corpus = corpus();
    let root = corpus["rootDir"].as_str().unwrap();
    for case in corpus["stamps"].as_array().unwrap() {
        assert_eq!(
            memory::stamp_origin(
                case["content"].as_str().unwrap(),
                root,
                case["filePath"].as_str().unwrap(),
                "sess_1",
                "/work/repo",
            ),
            case["stamped"].as_str().unwrap(),
            "{case}"
        );
    }
}
