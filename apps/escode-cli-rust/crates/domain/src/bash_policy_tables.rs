//! Bash 只读策略表的 serde 形状（rust-unified-host.md P1 #11）：`bash_policies.json`
//! 由生成器从 TS 导出，这里是它的**唯一**解析入口——`deny_unknown_fields` 让 TS 侧新增
//! 字段在加载期显式失败（替代原先 `Value` DOM 下标 + 静默忽略）；`safeFlags` 的 kind
//! 直接反序列化为 [`SafeFlagKind`]，未知 kind 同样是加载期错误而不是运行期的静默不匹配。

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use crate::bash_policy::{Policy, Tables};
use crate::bash_safe_flag::SafeFlagKind;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegexJson {
    pub source: String,
    /// TS 侧 `RegExp.flags`；Rust 侧只支持无 flag 的 source（生成器当前不使用 flag）。
    #[serde(default)]
    pub flags: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PolicyJson {
    /// None 与空表语义不同：TS 无 safeFlags 字段时直接拒绝，空对象仍允许位置参数。
    #[serde(default)]
    pub safe_flags: Option<BTreeMap<String, SafeFlagKind>>,
    #[serde(default)]
    pub allow_any_args: bool,
    #[serde(default)]
    pub allow_compact_numeric_count_flag: bool,
    #[serde(default)]
    pub command_only: bool,
    #[serde(default)]
    pub respects_double_dash: Option<bool>,
    #[serde(default)]
    pub regex: Option<RegexJson>,
    #[serde(default)]
    pub callback: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TablesJson {
    pub commands: Vec<(String, PolicyJson)>,
    pub multiword: Vec<(String, PolicyJson)>,
    pub git_subcommands: Vec<(String, PolicyJson)>,
    pub allow_any_arg_commands: Vec<String>,
    pub allow_any_arg_command_prefixes: Vec<Vec<String>>,
    pub git_global_no_value_flags: Vec<String>,
    pub git_global_value_flags: Vec<String>,
    pub git_global_dangerous_flags: Vec<String>,
}

impl PolicyJson {
    pub(crate) fn into_policy(self) -> Policy {
        Policy {
            safe_flags: self.safe_flags.map(|flags| flags.into_iter().collect()),
            allow_any_args: self.allow_any_args,
            allow_compact_numeric: self.allow_compact_numeric_count_flag,
            command_only: self.command_only,
            respects_double_dash: self.respects_double_dash,
            regex: self.regex.map(|regex| {
                assert!(
                    regex.flags.is_empty(),
                    "generated Bash policy regex uses unsupported flags: {}",
                    regex.flags
                );
                regex::Regex::new(&regex.source).expect("generated Bash policy regex")
            }),
            callback: self.callback,
        }
    }
}

impl TablesJson {
    /// TS 按前缀词数降序做稳定排序；同长度保持表内原序。
    fn ordered(entries: Vec<(String, PolicyJson)>) -> Vec<(String, Policy)> {
        let mut entries: Vec<(String, Policy)> = entries
            .into_iter()
            .map(|(key, policy)| (key, policy.into_policy()))
            .collect();
        entries.sort_by_key(|(key, _)| std::cmp::Reverse(key.split(' ').count()));
        entries
    }

    pub(crate) fn into_tables(self) -> Tables {
        Tables {
            commands: self
                .commands
                .into_iter()
                .map(|(key, policy)| (key, policy.into_policy()))
                .collect::<HashMap<_, _>>(),
            multiword: Self::ordered(self.multiword),
            git: Self::ordered(self.git_subcommands),
            allow_any: self.allow_any_arg_commands.into_iter().collect(),
            allow_any_prefixes: self.allow_any_arg_command_prefixes,
            git_no_value: self.git_global_no_value_flags.into_iter().collect(),
            git_value: self.git_global_value_flags.into_iter().collect(),
            git_dangerous: self.git_global_dangerous_flags,
        }
    }
}
