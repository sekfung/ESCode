//! 设置同步支持的外部 agent（TS `SettingsSyncAgent`）。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    CodexCli,
    OpenCode,
    OpenClaw,
    Augment,
    Continue,
    Goose,
    QwenCode,
    Qode,
    QodeCn,
    Windsurf,
    Trae,
    TraeCn,
    KiroCli,
    Roo,
    CodeBuddy,
    Agents,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claudeCode",
            Self::CodexCli => "codexCli",
            Self::OpenCode => "openCode",
            Self::OpenClaw => "openClaw",
            Self::Augment => "augment",
            Self::Continue => "continue",
            Self::Goose => "goose",
            Self::QwenCode => "qwenCode",
            Self::Qode => "qode",
            Self::QodeCn => "qodeCn",
            Self::Windsurf => "windsurf",
            Self::Trae => "trae",
            Self::TraeCn => "traeCn",
            Self::KiroCli => "kiroCli",
            Self::Roo => "roo",
            Self::CodeBuddy => "codeBuddy",
            Self::Agents => "agents",
        }
    }
}
