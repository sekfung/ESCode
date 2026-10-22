use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSnapshot {
    pub cwd: String,
    pub platform: String,
    pub shell: String,
    pub os_version: String,
    pub current_date: String,
    pub git: Option<GitSnapshot>,
    /// SVN 工作副本快照（docs/svn/architecture.md「Agent 集成」）；与 `git` 互斥。
    #[serde(default)]
    pub svn: Option<SvnSnapshot>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitSnapshot {
    pub branch: String,
    pub main_branch: String,
    pub user: String,
    pub status: String,
    pub recent_commits: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SvnSnapshot {
    pub url: String,
    pub revision: Option<u64>,
    pub branch: String,
    pub status: String,
    pub recent_logs: String,
}
pub struct InstructionSource {
    pub path: String,
    pub user: bool,
    pub content: String,
    pub truncated: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Templates {
    user_steer: String,
    date_change: String,
    cli: String,
    identity: String,
    desktop: String,
    desktop_gen_ui: String,
    behavior: String,
    context_management: String,
}
fn templates() -> &'static Templates {
    static TEMPLATES: OnceLock<Templates> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        serde_json::from_str(include_str!("prompt_templates.json"))
            .expect("validated prompt assets")
    })
}
pub fn user_steer(text: &str) -> String {
    templates()
        .user_steer
        .replacen("{escode_input_text}", text, 1)
}
/// TS `injectDateChangeReminderIntoMessageHistory`：同一运行时内本地日期变化后，下一轮次前插入的
/// reminder（docs/specs/rust-date-change.md）。首轮只记录日期；日期未变时不插入。
pub fn date_change_reminder(previous: Option<&str>, current: &str) -> Option<Value> {
    let previous = previous.filter(|p| *p != current)?;
    let body = templates()
        .date_change
        .replacen("{previous}", previous, 1)
        .replacen("{date}", current, 1);
    Some(serde_json::json!({
        "role": "user",
        "content": super::plan_mode::wrap(&body),
        "_escode_source": "date_change",
    }))
}
/// TS `withConversationQuotes` 的引用段；完整实现见 [`crate::conversation_input`]。
pub fn with_conversation_quotes(text: &str, quotes: &Value) -> String {
    crate::conversation_input::with_conversation_material(text, quotes)
}

#[allow(clippy::too_many_arguments)]
pub fn prefix(
    snapshot: &PromptSnapshot,
    sources: &[InstructionSource],
    model: Option<(&str, &str)>,
    desktop: bool,
    gen_ui_directory: Option<&str>,
    skill_guidance: bool,
    memory: Option<(&str, Option<&str>)>,
    actor_identity: Option<&str>,
) -> Vec<Value> {
    let templates = templates();
    // TS buildDesktopContextSection(genUiOutputDirectory)：只有 Host 提供并已创建会话目录时才声明 Gen UI。
    let stable = match (desktop, gen_ui_directory) {
        (true, Some(directory)) => format!(
            "{}\n\n{}",
            templates.identity,
            templates.desktop_gen_ui.replacen(
                "\"{genUiDirectory}\"",
                &serde_json::to_string(directory).unwrap_or_default(),
                1,
            )
        ),
        (true, None) => format!("{}\n\n{}", templates.identity, templates.desktop),
        (false, _) => templates.identity.clone(),
    };
    let mut env = format!(
        "# Environment\nYou have been invoked in the following environment:\n- Primary working directory: {}\n- Is a git repository: {}\n- Platform: {}\n- Shell: {}\n- OS Version: {}",
        snapshot.cwd,
        if snapshot.git.is_some() { "yes" } else { "no" },
        snapshot.platform,
        snapshot.shell,
        snapshot.os_version,
    );
    if snapshot.svn.is_some() {
        // 与 TS env-info 一致：Git 行保留，SVN 工作副本额外追加一行。
        let at = env.find("\n- Platform:").unwrap_or(env.len());
        env.insert_str(at, "\n- Is an SVN working copy: yes");
    }
    if let Some((provider, model)) = model {
        env.push_str(&format!(
            "\n- You are powered by the model named {provider}/{model}."
        ));
    }
    // TS buildSessionGuidanceSection：有 Skill 工具且存在技能时，在行为段后插入 Session-specific guidance。
    let guidance = if skill_guidance {
        "\n\n# Session-specific guidance\n- When the user types `/<skill-name>`, invoke it via Skill. Only use skills listed in the user-invocable skills section \u{2014} don't guess."
    } else {
        ""
    };
    // TS builder：Memory 段位于 Session-specific guidance 之后、Environment 之前（docs/specs/rust-project-memory.md）。
    let memory_section = memory
        .map(|(root, _)| format!("\n\n{}", super::memory::section(root)))
        .unwrap_or_default();
    // TS ContextBuilder 的工作流 actor 路径：无行为段与会话指引（它们面向「与用户对话」）。
    let mut dynamic = match actor_identity {
        Some(_) => format!(
            "{memory_section}\n\n{env}\n\n{}",
            templates.context_management
        ),
        None => format!(
            "\n\n{}{guidance}{memory_section}\n\n{env}\n\n{}",
            templates.behavior, templates.context_management
        ),
    };
    if let Some(git) = &snapshot.git {
        dynamic.push_str("\n\ngitStatus: This is the git status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation.");
        for (label, value) in [
            ("Current branch", &git.branch),
            (
                "Main branch (you will usually use this for PRs)",
                &git.main_branch,
            ),
            ("Git user", &git.user),
        ] {
            if !value.is_empty() {
                dynamic.push_str(&format!("\n\n{label}: {value}"));
            }
        }
        dynamic.push_str(&format!(
            "\n\nStatus:\n{}\n\nRecent commits:\n{}",
            if git.status.is_empty() {
                "(clean)"
            } else {
                &git.status
            },
            git.recent_commits
        ));
    }
    if let Some(svn) = &snapshot.svn {
        dynamic.push_str(&svn_system_context(svn));
    }
    // 工作流 actor 没有 CLI 前缀，身份段换成 actor 身份（TS buildWorkflowActorIdentitySection）。
    // actor 的身份段与动态段在 TS 里合成同一条 system 消息。
    let actor_system = actor_identity.map(|identity| format!("{identity}{dynamic}"));
    let system: Vec<&str> = match &actor_system {
        Some(merged) => vec![merged],
        None => vec![&templates.cli, &stable, &dynamic],
    };
    let mut messages = system
        .into_iter()
        .map(|content| {
            json!({"role":"system","content":content,"_escode_cache_control":{"type":"ephemeral"}})
        })
        .collect::<Vec<_>>();
    let mut sections = vec![];
    for source in sources {
        let body = if source.truncated {
            format!("{}\n\n[File truncated: AGENTS.md]", source.content)
        } else {
            source.content.clone()
        };
        let body = body.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if !body.is_empty() {
            sections.push(format!(
                "Contents of {} ({}):\n\n{body}",
                source.path,
                if source.user {
                    "user default instructions"
                } else {
                    "workspace instructions"
                }
            ));
        }
    }
    // MEMORY.md 索引与 AGENTS.md 同处 agentsMd 块末尾（TS buildRequestUserContextContent）。
    if let Some(index) = memory.and_then(|(_, index)| index) {
        sections.push(index.to_owned());
    }
    let mut context = vec![];
    if !sections.is_empty() {
        context.push(format!("# agentsMd\nCodebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.\n\n{}", sections.join("\n\n")));
    }
    if !snapshot.current_date.is_empty() {
        context.push(format!(
            "# currentDate\nToday's date is {}.",
            snapshot.current_date
        ));
    }
    if !context.is_empty() {
        let body = format!(
            "As you answer the user's questions, you can use the following context:\n{}\n\n      IMPORTANT: this context may or may not be relevant to your tasks. You should not respond to this context unless it is highly relevant to your task.",
            context.join("\n\n")
        );
        static NESTED: OnceLock<regex::Regex> = OnceLock::new();
        let escaped = NESTED
            .get_or_init(|| regex::Regex::new(r"(?i)</?system-reminder\b").unwrap())
            .replace_all(&body, |caps: &regex::Captures<'_>| {
                format!("&lt;{}", &caps[0][1..])
            });
        messages.push(json!({"role":"user","content":format!("<system-reminder>\n{escaped}\n</system-reminder>\n")}));
    }
    messages
}

/// 与 TS `buildSvnSystemContextSection` 同文案；SVN 提交直接写入服务器，没有本地提交或推送。
fn svn_system_context(svn: &SvnSnapshot) -> String {
    let mut text = String::from(
        "\n\nsvnStatus: This is the SVN working copy status at the start of the conversation. Note that this status is a snapshot in time, and will not update during the conversation. SVN commits go directly to the server; there is no local commit or push.",
    );
    if !svn.url.is_empty() {
        text.push_str(&format!("\n\nRepository URL: {}", svn.url));
    }
    if !svn.branch.is_empty() {
        text.push_str(&format!("\n\nCurrent branch: {}", svn.branch));
    }
    if let Some(revision) = svn.revision {
        text.push_str(&format!("\n\nWorking copy revision: r{revision}"));
    }
    text.push_str(&format!(
        "\n\nStatus:\n{}\n\nRecent revisions:\n{}",
        if svn.status.is_empty() {
            "(clean)"
        } else {
            &svn.status
        },
        svn.recent_logs
    ));
    text
}

#[cfg(test)]
mod date_change_tests {
    use super::date_change_reminder;

    #[test]
    fn first_turn_and_same_day_do_not_remind() {
        assert!(date_change_reminder(None, "2026-09-26").is_none());
        assert!(date_change_reminder(Some("2026-09-26"), "2026-09-26").is_none());
    }

    #[test]
    fn date_change_uses_ts_body_wrapped_as_system_reminder() {
        let message = date_change_reminder(Some("2026-09-26"), "2026-09-27").unwrap();
        assert_eq!(message["role"], "user");
        assert_eq!(message["_escode_source"], "date_change");
        assert_eq!(
            message["content"],
            "<system-reminder>
The date has changed. Today's date is now 2026-09-27. DO NOT mention this to the user explicitly because they are already aware.
</system-reminder>"
        );
    }
}

#[cfg(test)]
mod conversation_quote_tests {
    use super::with_conversation_quotes;
    use serde_json::json;

    #[test]
    fn quotes_render_like_ts_with_conversation_quotes() {
        // 期望值来自 TS：parts.push(`<conversation_quote>\n${JSON.stringify(quotes)}\n</conversation_quote>`)。
        let quotes = json!([{"senderName": "s", "path": "p", "text": "a\"b"}]);
        assert_eq!(
            with_conversation_quotes("hi", &quotes),
            "hi\n\n<conversation_quote>\n[{\"text\":\"a\\\"b\",\"path\":\"p\",\"senderName\":\"s\"}]\n</conversation_quote>"
        );
        assert_eq!(with_conversation_quotes("hi", &json!([])), "hi");
        assert!(with_conversation_quotes("", &quotes).starts_with("<conversation_quote>"));
    }
}


#[cfg(test)]
mod svn_prompt_tests {
    use super::{PromptSnapshot, SvnSnapshot, prefix};

    #[test]
    fn svn_snapshot_renders_environment_line_and_system_context() {
        let snapshot = PromptSnapshot {
            cwd: "/workspace".into(),
            platform: "linux".into(),
            shell: "bash".into(),
            os_version: "linux 6.8 x64".into(),
            svn: Some(SvnSnapshot {
                url: "https://svn.example.invalid/repo/trunk".into(),
                revision: Some(120),
                branch: "trunk".into(),
                status: "M       src/a.ts".into(),
                recent_logs: "r120 alice: fix login".into(),
            }),
            ..Default::default()
        };
        let messages = prefix(&snapshot, &[], None, false, None, false, None, None);
        let dynamic = messages
            .iter()
            .filter_map(|message| message["content"].as_str())
            .find(|content| content.contains("# Environment"))
            .expect("environment section");
        assert!(dynamic.contains(
            "- Is a git repository: no\n- Is an SVN working copy: yes\n- Platform: linux"
        ));
        assert!(dynamic.contains(
            "svnStatus: This is the SVN working copy status at the start of the conversation."
        ));
        assert!(dynamic.contains("\n\nRepository URL: https://svn.example.invalid/repo/trunk\n\nCurrent branch: trunk\n\nWorking copy revision: r120\n\nStatus:\nM       src/a.ts\n\nRecent revisions:\nr120 alice: fix login"));
        assert!(!dynamic.contains("gitStatus:"));
    }
}
