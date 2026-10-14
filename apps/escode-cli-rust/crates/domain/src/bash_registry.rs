//! Bash 命令注册表（fig 命令树）的类型化形状与完整性断言（rust-unified-host.md P1 #11）。
//!
//! 语料 `bash_command_registry.json` 由 `scripts/generate-escode-cli-rust-bash-policies.mjs`
//! 从 TS 的 `BASH_COMMAND_REGISTRY` 导出。节点是位置数组
//! `[names, options, argFlags, children]`；这里把它反序列化成带名字的结构体，
//! 前缀解析不再对 `Value` 做下标访问（TS 侧形状变化在加载期显式失败）。
//!
//! 完整性：语料带 `version` 与 `hash`（`commands` 的键排序规范 JSON 的 sha256）。
//! `version` 在加载期断言，`hash` 由测试重算比对——手改或半截更新都会被拦住。

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
#[cfg(test)]
use serde_json::Value;

/// 语料结构版本：生成器与 Rust 同步升级（不一致即加载失败）。
pub(crate) const EXPECTED_REGISTRY_VERSION: u32 = 1;

/// 一个命令节点的选项规格：`[[flagNames...], takesValue(0|1)]`。
#[derive(Clone, Debug)]
pub(crate) struct OptionSpec {
    pub names: Vec<String>,
    /// 1 = 该选项带值（`--flag value` 消耗两个 token）。
    pub takes_value: u64,
}

impl<'de> Deserialize<'de> for OptionSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (names, takes_value): (Vec<String>, u64) = Deserialize::deserialize(deserializer)?;
        Ok(Self { names, takes_value })
    }
}

/// 一个命令节点：`[names, options, argFlags, children]`。
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub names: Vec<String>,
    pub options: Vec<OptionSpec>,
    pub arg_flags: u64,
    pub children: Vec<Node>,
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (names, options, arg_flags, children): (Vec<String>, Vec<OptionSpec>, u64, Vec<Node>) =
            Deserialize::deserialize(deserializer)?;
        Ok(Self {
            names,
            options,
            arg_flags,
            children,
        })
    }
}

impl Node {
    /// 子节点里名称命中 `token` 的那个。
    pub fn child(&self, token: &str) -> Option<&Node> {
        self.children
            .iter()
            .find(|child| child.names.iter().any(|name| name == token))
    }

    /// 当前节点下名为 `name` 的选项规格。
    pub fn option(&self, name: &str) -> Option<&OptionSpec> {
        self.options
            .iter()
            .find(|option| option.names.iter().any(|flag| flag == name))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RegistryJson {
    version: u32,
    /// 内容 hash：加载期不校验（解析已保证形状），由 `registry_version_and_hash_match_the_generator`
    /// 重算比对——手改或半截更新都会被测试拦住。
    #[cfg_attr(not(test), allow(dead_code))]
    hash: String,
    commands: BTreeMap<String, Node>,
}

/// 已校验的注册表：命令名 → 节点（O(1) 查找，替代原先的 `Value` 线性遍历）。
pub(crate) struct Registry {
    pub commands: HashMap<String, Node>,
}

impl Registry {
    pub fn get(&self, name: &str) -> Option<&Node> {
        self.commands.get(name)
    }
}

pub(crate) fn parse() -> Registry {
    let raw: RegistryJson = serde_json::from_str(include_str!("bash_command_registry.json"))
        .expect("generated Bash command registry");
    assert_eq!(
        raw.version, EXPECTED_REGISTRY_VERSION,
        "Bash command registry version mismatch; regenerate with scripts/generate-escode-cli-rust-bash-policies.mjs"
    );
    let commands = raw.commands.into_iter().collect::<HashMap<String, Node>>();
    Registry { commands }
}

/// 语料里声明的 hash（测试比对用）。
#[cfg(test)]
pub(crate) fn declared_hash() -> String {
    let raw: RegistryJson = serde_json::from_str(include_str!("bash_command_registry.json"))
        .expect("generated Bash command registry");
    raw.hash
}

/// 规范 JSON 的 sha256：`commands` 按键排序序列化（serde_json 的 Map 为 BTreeMap，天然有序；
/// 生成器侧同样按键排序后计算）。
#[cfg(test)]
pub(crate) fn canonical_hash() -> String {
    use sha2::{Digest, Sha256};
    // commands 单独以 Value 解析：serde_json 的 Map 为 BTreeMap（键排序），
    // 与生成器 `Object.keys(...).sort()` 的规范形一致；数组保持原序。
    let raw: Value = serde_json::from_str(include_str!("bash_command_registry.json"))
        .expect("generated Bash command registry");
    let commands = raw
        .get("commands")
        .expect("registry carries commands")
        .clone();
    let text = serde_json::to_string(&commands).expect("registry serializes");
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_version_and_hash_match_the_generator() {
        assert_eq!(
            declared_hash(),
            canonical_hash(),
            "语料的 hash 必须匹配内容"
        );
        let registry = parse();
        assert!(registry.commands.len() > 100, "命令树规模");
        let git = registry.get("git").expect("git 在注册表里");
        assert!(git.names.contains(&"git".to_owned()));
        assert!(git.child("add").is_some(), "子命令按名查找");
        assert!(git.option("--git-dir").is_some(), "选项按名查找");
    }

    #[test]
    fn node_deserialization_rejects_positional_drift() {
        // 位置数组少一项 → 反序列化失败（TS 形状变化不会被静默接受）。
        let broken = "[\"git\",  []]";
        assert!(serde_json::from_str::<Node>(broken).is_err());
    }

    #[test]
    fn declared_value_is_not_free_form() {
        // 顶层出现生成器之外的字段（deny_unknown_fields）即失败。
        let broken = r#"{"version":1,"hash":"x","commands":{},"extra":true}"#;
        assert!(serde_json::from_str::<RegistryJson>(broken).is_err());
        let _ = Value::Null;
    }
}
