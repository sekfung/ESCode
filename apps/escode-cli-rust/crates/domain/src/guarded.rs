//! Guarded 模式的危险命令匹配（逐位对应 TS `core/src/tool/handlers/guarded/*`）。
//! guarded = YOLO 基础策略 + 明确危险命令的用户单次批准；本模块只判定「是否命中、命中哪条规则」，
//! 不展开变量、不执行。差分 oracle 为 tests/fixtures/guarded_corpus.json
//! （scripts/generate-escode-cli-rust-guarded-corpus.mjs）。见 docs/specs/rust-v3.15.1-drift.md。

pub(crate) use crate::guarded_options::{
    COMMAND_SCAN, OptionTable, ScanPolicy, WRAPPER_SCAN, enabled, parse_options, settle,
};
use crate::guarded_options::unwrap_command;
use crate::guarded_rules::{match_cmd_delete, match_find_delete, match_git, match_robocopy_delete};
pub(crate) use crate::guarded_word::command_basename;

/// TS `ExecutionShellSelection["dialect"]`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Posix,
    GitBash,
    Cmd,
    LegacyShell,
}

impl Dialect {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "posix" => Self::Posix,
            "git-bash" => Self::GitBash,
            "cmd" => Self::Cmd,
            "legacy-shell" => Self::LegacyShell,
            _ => return None,
        })
    }
}

/// TS `DangerousCommandResult`；`start`/`end` 是 UTF-16 偏移（与 TS 字符串下标一致）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Matched {
        rule_id: String,
        start: usize,
        end: usize,
    },
    NotMatched,
    Unsupported,
}

/// TS `CommandWord`：AST 定位后的单词事实。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandWord {
    pub value: String,
    pub dynamic: bool,
    pub has_unquoted_glob: bool,
    pub start: usize,
    pub end: usize,
}

/// TS `RuleMatch`：命中规则 / 已支持未命中（false）/ 超出文法（undefined）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    Hit(&'static str),
    Miss,
    Unknown,
}

const MAX_LENGTH: usize = 10_000;
const INTERPRETERS: [&str; 14] = [
    "bash",
    "sh",
    "zsh",
    "cmd",
    "powershell",
    "pwsh",
    "eval",
    "source",
    ".",
    "python",
    "python3",
    "node",
    "perl",
    "ruby",
];
const RSYNC_DELETE: [&str; 7] = [
    "--delete",
    "--del",
    "--delete-before",
    "--delete-during",
    "--delete-delay",
    "--delete-after",
    "--delete-excluded",
];

/// 匹配器可能产出的全部规则 id（`safety.bash.<rule>`）。权限判定的 rule id 是静态字符串，
/// guarded 的 `userApprovalRule` 经此表收敛；表外的 id 不是本匹配器产出，不触发单次确认。
pub const SAFETY_RULE_IDS: [&str; 13] = [
    "safety.bash.remove-critical-path",
    "safety.bash.system-storage",
    "safety.bash.rsync-delete",
    "safety.bash.git-reset-hard",
    "safety.bash.git-clean-force",
    "safety.bash.git-push-force",
    "safety.bash.git-discard-tree",
    "safety.bash.git-stash-clear",
    "safety.bash.git-worktree-force-remove",
    "safety.bash.find-delete",
    "safety.bash.cmd-remove-tree",
    "safety.bash.cmd-delete-files",
    "safety.bash.robocopy-delete",
];

pub fn static_rule_id(rule: &str) -> Option<&'static str> {
    SAFETY_RULE_IDS.iter().copied().find(|known| *known == rule)
}

/// TS `matchDangerousCommand`。
pub fn match_dangerous_command(command: &str, dialect: Dialect) -> Outcome {
    if command.encode_utf16().count() > MAX_LENGTH || dialect == Dialect::LegacyShell {
        return Outcome::Unsupported;
    }
    let windows = matches!(dialect, Dialect::Cmd | Dialect::GitBash);
    let (commands, mut unsupported) = if dialect == Dialect::Cmd {
        crate::guarded_cmd::analyze(command)
    } else {
        let analysis = crate::bash_parse::analyze(command);
        // 解析失败没有可依赖的定位；局部 unsupported 节点则不丢弃其它独立命令。
        if analysis.has_parse_errors {
            return Outcome::Unsupported;
        }
        let words = analysis
            .commands
            .into_iter()
            .map(|invocation| invocation.words)
            .collect();
        (words, analysis.has_unsupported_syntax)
    };
    for invocation in commands {
        let Some(words) = unwrap_command(&invocation, windows).filter(|w| !w.is_empty()) else {
            unsupported = true;
            continue;
        };
        // TS `name ? matchInvocation(...) : undefined`：空命令名（`rd/`）同样超出文法。
        let result = match command_basename(&words[0], windows).filter(|name| !name.is_empty()) {
            Some(name) => match_invocation(&name, &words[1..], dialect),
            None => Rule::Unknown,
        };
        if let Rule::Hit(rule) = result {
            return Outcome::Matched {
                rule_id: format!("safety.bash.{rule}"),
                start: words[0].start,
                end: words[words.len() - 1].end,
            };
        }
        if result == Rule::Unknown || words.iter().any(|word| word.dynamic) {
            unsupported = true;
        }
    }
    if unsupported {
        Outcome::Unsupported
    } else {
        Outcome::NotMatched
    }
}

fn match_invocation(name: &str, args: &[CommandWord], dialect: Dialect) -> Rule {
    if name == "git" {
        return match_git(args);
    }
    if INTERPRETERS.contains(&name) {
        return Rule::Unknown;
    }
    if dialect == Dialect::Cmd && ["rd", "rmdir", "del", "erase"].contains(&name) {
        return match_cmd_delete(name, args);
    }
    if dialect != Dialect::Cmd && name == "find" {
        return match_find_delete(args);
    }
    if matches!(dialect, Dialect::Cmd | Dialect::GitBash) && name == "robocopy" {
        return match_robocopy_delete(args, dialect);
    }
    if name == "rm" {
        let table = OptionTable::new(
            "-r -R --recursive -f --force -i -I --one-file-system --preserve-root --no-preserve-root -v --verbose -d --dir --help --version",
            "",
            "--interactive",
        );
        let parsed = parse_options(args, &table, COMMAND_SCAN);
        // BSD rm 在首个目标后不再解析选项；目标计数单独按该语义扫描，危险选项仍保留 GNU 的交错扫描。
        let targets = parse_options(
            args,
            &table,
            ScanPolicy {
                stop_at_operand: true,
                abort_on_unknown: false,
            },
        );
        let hit = enabled(&parsed.flags, &["-r", "-R", "--recursive"], &[])
            || targets.operands.len() >= 2
            || targets.operands.iter().any(|word| word.has_unquoted_glob);
        return settle(hit.then_some("remove-critical-path"), &parsed);
    }
    let mkfs_variant = name.strip_prefix("mkfs.").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    });
    if ["dd", "mount", "umount", "mkfs"].contains(&name) || mkfs_variant {
        // 只豁免独立且明确的查询形式，不能把 -o/其它参数值中的 --help 当成退出选项。
        if args.len() == 1 && !args[0].dynamic && ["--help", "--version"].contains(&args[0].value.as_str()) {
            return Rule::Miss;
        }
        if name == "mount" && args.is_empty() {
            return Rule::Miss;
        }
        return Rule::Hit("system-storage");
    }
    if name == "rsync" {
        let table = OptionTable::new(
            &format!(
                "-a -r -v -q -z -h -P -p -t -g -o -D -l -H -A -X -u -c -n --dry-run --archive --recursive --verbose --quiet --compress --progress --partial --checksum --update --numeric-ids --ignore-errors --force --help --version {}",
                RSYNC_DELETE.join(" ")
            ),
            "-e --rsh -f --filter --include --exclude --include-from --exclude-from --files-from --rsync-path --log-file --out-format --backup-dir --suffix --chmod --chown --max-delete --timeout --bwlimit --compare-dest --copy-dest --link-dest",
            "",
        );
        let parsed = parse_options(args, &table, COMMAND_SCAN);
        let hit = !parsed
            .flags
            .iter()
            .any(|f| ["-n", "--dry-run", "--help", "--version"].contains(&f.as_str()))
            && parsed.flags.iter().any(|f| RSYNC_DELETE.contains(&f.as_str()));
        return settle(hit.then_some("rsync-delete"), &parsed);
    }
    Rule::Miss
}

