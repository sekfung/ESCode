//! Guarded 的有限选项文法与包装命令展开（TS guarded/options.ts、wrappers.ts）。

use crate::guarded::{CommandWord, Rule};
use crate::guarded_word::command_basename;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arity {
    Flag,
    Value,
    Optional,
}

pub(crate) struct OptionTable(Vec<(String, Arity)>);

impl OptionTable {
    /// TS `optionTable(flags, values, optional)`：后声明的同名项覆盖先声明的（Object.fromEntries）。
    pub(crate) fn new(flags: &str, values: &str, optional: &str) -> Self {
        let mut entries: Vec<(String, Arity)> = Vec::new();
        for (names, arity) in [
            (flags, Arity::Flag),
            (values, Arity::Value),
            (optional, Arity::Optional),
        ] {
            for name in names.split_whitespace() {
                entries.retain(|(known, _)| known != name);
                entries.push((name.to_owned(), arity));
            }
        }
        Self(entries)
    }
    fn get(&self, name: &str) -> Option<Arity> {
        self.0
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, arity)| *arity)
    }
}

/// TS `OptionScanPolicy`。
#[derive(Clone, Copy)]
pub(crate) struct ScanPolicy {
    pub stop_at_operand: bool,
    pub abort_on_unknown: bool,
}
pub(crate) const WRAPPER_SCAN: ScanPolicy = ScanPolicy {
    stop_at_operand: true,
    abort_on_unknown: true,
};
pub(crate) const COMMAND_SCAN: ScanPolicy = ScanPolicy {
    stop_at_operand: false,
    abort_on_unknown: false,
};

#[derive(Default)]
pub(crate) struct Parsed {
    pub flags: Vec<String>,
    pub operands: Vec<CommandWord>,
    pub unsupported: bool,
}

/// TS `parseOptions`：先消耗已声明 arity 的参数值；未知选项不猜 arity。
pub(crate) fn parse_options(words: &[CommandWord], table: &OptionTable, policy: ScanPolicy) -> Parsed {
    let mut result = Parsed::default();
    let mut ended = false;
    let mut i = 0;
    while i < words.len() {
        let word = &words[i];
        if !ended && !word.dynamic && word.value == "--" {
            ended = true;
            i += 1;
            continue;
        }
        if ended || word.dynamic || !word.value.starts_with('-') || word.value == "-" {
            result.operands.push(word.clone());
            if policy.stop_at_operand {
                result.operands.extend(words[i + 1..].iter().cloned());
                break;
            }
            i += 1;
            continue;
        }
        let long = word.value.starts_with("--");
        let equals = word.value.find('=');
        let names: Vec<String> = if long {
            vec![equals.map_or(word.value.clone(), |at| word.value[..at].to_owned())]
        } else {
            word.value.chars().skip(1).map(|c| format!("-{c}")).collect()
        };
        for (n, name) in names.iter().enumerate() {
            let arity = table.get(name);
            let attached = if long { equals.is_some() } else { n < names.len() - 1 };
            // 未知选项只标记 unsupported、继续扫描，不能把已识别的 -r/-f 一并丢弃；abort 仅给 wrapper。
            if arity.is_none() || (arity == Some(Arity::Flag) && long && attached) {
                result.unsupported = true;
                if policy.abort_on_unknown {
                    return result;
                }
                continue;
            }
            result.flags.push(name.clone());
            if arity == Some(Arity::Value) && !attached {
                i += 1;
                if i >= words.len() {
                    result.unsupported = true;
                }
            }
            if arity != Some(Arity::Flag) {
                break;
            }
        }
        i += 1;
    }
    result
}

/// TS `enabled`：最后出现的开/关选项生效。
pub(crate) fn enabled(flags: &[String], yes: &[&str], no: &[&str]) -> bool {
    let mut result = false;
    for flag in flags {
        if yes.contains(&flag.as_str()) {
            result = true;
        }
        if no.contains(&flag.as_str()) {
            result = false;
        }
    }
    result
}

/// TS `settleRule`：命中优先；否则遇到过未知选项为 unsupported，否则为已支持未命中。
pub(crate) fn settle(rule: Option<&'static str>, parsed: &Parsed) -> Rule {
    match rule {
        Some(rule) => Rule::Hit(rule),
        None if parsed.unsupported => Rule::Unknown,
        None => Rule::Miss,
    }
}

/// TS `unwrapCommand`：剥掉 env / time / sudo 包装，猜不出真实命令起点时整体放弃。
pub(crate) fn unwrap_command(input: &[CommandWord], windows: bool) -> Option<Vec<CommandWord>> {
    let mut words = input.to_vec();
    while !words.is_empty() {
        let name = command_basename(&words[0], windows)?;
        let table = match name.as_str() {
            "env" => OptionTable::new(
                "-i --ignore-environment -0 --null -v --debug",
                "-u --unset -C --chdir",
                "",
            ),
            "time" => OptionTable::new(
                "-a --append -p --portability -v --verbose -q --quiet",
                "-o --output -f --format",
                "",
            ),
            "sudo" => OptionTable::new(
                "-A --askpass -b --background -E --preserve-env -H --set-home -K --remove-timestamp -k --reset-timestamp -n --non-interactive -S --stdin",
                "-u --user -g --group -h --host -p --prompt -C --close-from -T --command-timeout -D --chdir -R --chroot",
                "",
            ),
            _ => return Some(words),
        };
        let parsed = parse_options(&words[1..], &table, WRAPPER_SCAN);
        if parsed.unsupported {
            return None;
        }
        words = parsed.operands;
        if name == "env" || name == "sudo" {
            while words.first().is_some_and(|word| is_env_assignment(&word.value)) {
                words.remove(0);
            }
        }
    }
    None
}

fn is_env_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
