//! `skills` 域（docs/specs/rust-u2-skills.md；TS 真相源 `packages/services/src/skills/`）。

mod discover;
pub mod frontmatter;
mod plugin_roots;
pub(crate) mod policy;
mod service;
mod service_support;
pub(crate) mod walk;

pub use discover::Compare;
pub use service::{SkillsError, SkillsService};
pub(crate) use walk::walk_markdown_paths;
