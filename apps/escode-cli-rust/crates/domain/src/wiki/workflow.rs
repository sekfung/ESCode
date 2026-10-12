//! 任务工作流识别与各工作流下的知识类型权重（spec §9.1）。
//! 只看用户本轮输入（展开前），关键词命中计分，平局按 review > debugging > testing > refactoring > coding。
use super::entry::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workflow {
    Coding,
    Debugging,
    Testing,
    Refactoring,
    Review,
    General,
}

impl Workflow {
    pub const ALL: [Workflow; 6] = [
        Workflow::Coding,
        Workflow::Debugging,
        Workflow::Testing,
        Workflow::Refactoring,
        Workflow::Review,
        Workflow::General,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Workflow::Coding => "coding",
            Workflow::Debugging => "debugging",
            Workflow::Testing => "testing",
            Workflow::Refactoring => "refactoring",
            Workflow::Review => "review",
            Workflow::General => "general",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.as_str() == text)
    }
}

const REVIEW: &[&str] = &[
    "review",
    "code review",
    "cr ",
    "pull request",
    "merge request",
    "diff",
    "审查",
    "评审",
    "审阅",
    "代码检视",
    "检视",
    "走查",
    "/review",
    "/code-review",
];
// 强信号是表达意图的动词/现象，弱信号是可能只出现在主题里的名词（如 "error handling" 里的 error）。
const DEBUGGING_STRONG: &[&str] = &[
    "bug",
    "debug",
    "crash",
    "panic",
    "exception",
    "stack trace",
    "traceback",
    "fails",
    "failing",
    "broken",
    "not working",
    "regression",
    "fix",
    "报错",
    "崩溃",
    "调试",
    "排查",
    "修复",
];
const DEBUGGING_WEAK: &[&str] = &["error", "errors", "错误", "失败", "问题"];
const TESTING_STRONG: &[&str] = &[
    "test",
    "tests",
    "unit test",
    "e2e",
    "coverage",
    "测试",
    "单测",
    "用例",
    "覆盖率",
];
const TESTING_WEAK: &[&str] = &["spec", "assert", "mock"];
const REFACTORING_STRONG: &[&str] = &[
    "refactor",
    "restructure",
    "clean up",
    "cleanup",
    "rename",
    "simplify",
    "decouple",
    "重构",
    "整理",
    "拆分",
    "解耦",
    "重命名",
];
const REFACTORING_WEAK: &[&str] = &["extract"];
const CODING_STRONG: &[&str] = &[
    "implement",
    "add",
    "create",
    "build",
    "write",
    "feature",
    "support",
    "实现",
    "新增",
    "添加",
    "编写",
    "开发",
    "增加",
    "支持",
];
const CODING_WEAK: &[&str] = &[
    "change", "update", "modify", "endpoint", "api", "function", "class", "module", "修改", "接口",
    "功能",
];

fn hits(text: &str, words: &[&str]) -> usize {
    words.iter().filter(|w| contains_word(text, w)).count()
}

/// ASCII 词按词边界匹配（避免 "address" 命中 "add"）；含非 ASCII 的词按子串匹配。
fn contains_word(text: &str, word: &str) -> bool {
    if !word.is_ascii() || word.starts_with('/') || word.ends_with(' ') {
        return text.contains(word);
    }
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(pos) = text[start..].find(word) {
        let at = start + pos;
        let end = at + word.len();
        let before = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        let after = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before && after {
            return true;
        }
        start = at + 1;
    }
    false
}

pub fn classify(prompt: &str) -> Workflow {
    let text = format!("{} ", prompt.to_lowercase());
    let score = |strong: &[&str], weak: &[&str]| hits(&text, strong) * 2 + hits(&text, weak);
    let scored = [
        (Workflow::Review, hits(&text, REVIEW) * 3),
        (Workflow::Debugging, score(DEBUGGING_STRONG, DEBUGGING_WEAK)),
        (Workflow::Testing, score(TESTING_STRONG, TESTING_WEAK)),
        (
            Workflow::Refactoring,
            score(REFACTORING_STRONG, REFACTORING_WEAK),
        ),
        (Workflow::Coding, score(CODING_STRONG, CODING_WEAK)),
    ];
    let best = scored
        .iter()
        .copied()
        .max_by(|a, b| a.1.cmp(&b.1).then(order(b.0).cmp(&order(a.0))));
    match best {
        Some((workflow, score)) if score > 0 => workflow,
        _ => Workflow::General,
    }
}

fn order(workflow: Workflow) -> u8 {
    match workflow {
        Workflow::Review => 0,
        Workflow::Debugging => 1,
        Workflow::Testing => 2,
        Workflow::Refactoring => 3,
        Workflow::Coding => 4,
        Workflow::General => 5,
    }
}

/// 工作流 × 类型权重：未列出的类型为 1.0。
pub fn kind_weight(workflow: Workflow, kind: Kind) -> f64 {
    use Kind::*;
    match (workflow, kind) {
        (Workflow::Review, Standard | Invariant | AntiPattern) => 1.5,
        (Workflow::Review, Contract | Adr) => 1.35,
        (Workflow::Review, Guide | Reference) => 0.9,
        (Workflow::Coding, Standard | Contract | Architecture) => 1.3,
        (Workflow::Coding, Pattern | Guide) => 1.2,
        (Workflow::Debugging, Invariant | Contract | AntiPattern) => 1.35,
        (Workflow::Debugging, Architecture | Guide) => 1.2,
        (Workflow::Testing, Guide | Standard) => 1.35,
        (Workflow::Testing, Contract | Invariant) => 1.2,
        (Workflow::Refactoring, Architecture | Adr | Pattern | AntiPattern) => 1.35,
        (Workflow::Refactoring, Standard) => 1.2,
        (_, Proposal) => 0.8,
        _ => 1.0,
    }
}

/// 相关性门槛倍数：闲聊/通用问题需要更强的命中才注入。
pub fn threshold_factor(workflow: Workflow) -> f64 {
    if workflow == Workflow::General {
        1.5
    } else {
        1.0
    }
}
