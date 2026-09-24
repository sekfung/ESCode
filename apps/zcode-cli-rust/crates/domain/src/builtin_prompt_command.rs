//! 内置提示词命令展开：对应 TS `bootstrap/builtin-prompt-command.ts` 的
//! `resolveZCodeBuiltinPromptCommand`（`/init` 展开为一个普通用户提示词）。
//! 已知差异：Rust 无动态工作流，`/workflow` 一律按关闭处理（TS 在 `dynamicWorkflowEnabled === false` 时同样返回 None）。
use std::path::Path;

/// TS `BUILTIN_PROMPT_COMMAND_PATTERN = /^\/([^\s]+)(?:\s+([\s\S]*))?$/` 作用于 `input.trim()`。
pub fn resolve_builtin_prompt_command(input: &str, working_directory: &Path) -> Option<String> {
    let (name, args) = parse_invocation(input.trim())?;
    if name.to_lowercase() != "init" {
        return None;
    }
    Some(build_init_agents_prompt(&args, working_directory))
}

fn parse_invocation(input: &str) -> Option<(String, String)> {
    let rest = input.strip_prefix('/')?;
    // 命令名到第一个空白为止；其余为参数（保留换行，按 TS 一样在两侧 trim）。
    let end = rest
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty() {
        return None;
    }
    let args = rest[end..].trim();
    Some((name.to_owned(), args.to_owned()))
}

fn build_init_agents_prompt(args: &str, working_directory: &Path) -> String {
    let target_path = working_directory.join("AGENTS.md");
    let hidden = working_directory.join(".zcode").join("AGENTS.md");
    let hidden_alt = working_directory.join(".agents").join("AGENTS.md");
    let additional = if args.is_empty() {
        String::new()
    } else {
        [
            "",
            "Additional user instructions supplied with /init:",
            "```text",
            args,
            "```",
        ]
        .join("\n")
    };
    [
        "You are running ZCode's built-in /init command.".into(),
        String::new(),
        "Your task is to create or update a concise workspace instruction file for future ZCode agents."
            .into(),
        String::new(),
        "Target:".into(),
        format!("- Workspace directory: {}", working_directory.display()),
        format!("- Instruction file: {}", target_path.display()),
        format!(
            "- Existing hidden instruction candidates: {} and {}",
            hidden.display(),
            hidden_alt.display()
        ),
        "- File name must be exactly AGENTS.md.".into(),
        "- This command targets the current workspace only. Do not write ~/.zcode/AGENTS.md.".into(),
        additional,
        String::new(),
        "Process:".into(),
        "1. First check whether .zcode/AGENTS.md or .agents/AGENTS.md exists in the workspace. If either exists, tell the user they already have an instructions file, mention the path found, and stop without creating a new AGENTS.md.".into(),
        "2. Inspect the repository before writing. Prefer Read, Glob, Grep, and safe Bash commands such as ls, find, git status, and package-manager script inspection.".into(),
        "3. If AGENTS.md already exists, read it first and update it with Edit instead of replacing it wholesale.".into(),
        "4. If AGENTS.md does not exist, create it at the workspace root.".into(),
        "5. Keep the file practical and short enough for future agents to read quickly.".into(),
        "6. Include only project-specific facts future ZCode agents would otherwise miss.".into(),
        "7. Ask the user only if a repository-specific decision cannot be inferred and would materially change the file.".into(),
        String::new(),
        "Recommended AGENTS.md content:".into(),
        "- Repository purpose and major directories.".into(),
        "- Build, typecheck, lint, and focused test commands discovered from the repo.".into(),
        "- Architecture boundaries and layer rules that matter for edits.".into(),
        "- Coding conventions, import/path rules, logging rules, UI/design rules, and platform compatibility constraints if present.".into(),
        "- Known gotchas for desktop app, web, remote, stdio, protocols, or agent runtime if this repo has them.".into(),
        "- Any documentation files that agents should read before changing sensitive areas.".into(),
        String::new(),
        "After creating or editing AGENTS.md, summarize the main sections you wrote and mention the file path.".into(),
    ]
    .join("\n")
}
