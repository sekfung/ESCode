//! LLM Wiki 的纯领域规则（apps/escode-cli-rust/docs/llm-wiki/01-spec.md）。
//! 不做 IO：存储、解析器、Git 与进程调用在 `escode-cli-wiki` 适配器。
pub mod config;
pub mod conflicts;
pub mod entry;
pub mod extract;
pub mod frontmatter;
pub mod graph;
pub mod index;
pub mod ir;
pub mod lint;
pub mod project;
pub mod rank;
pub mod reminder;
pub mod source;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tests_extract;
#[cfg(test)]
mod tests_governance;
pub mod text;
pub mod tools;
pub mod workflow;
