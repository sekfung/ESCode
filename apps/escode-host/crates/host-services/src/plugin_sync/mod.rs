//! `plugin-sync` 服务域（docs/specs/rust-u2-plugin-sync.md；TS `packages/services/src/plugin-sync/`）。

mod archive;
mod archive_tar;
mod candidates;
mod config;
mod import;
mod marketplace;
mod marketplace_import;
mod marketplace_manifest;
mod paths;
mod service;

#[cfg(test)]
mod tests;

pub use service::{
    DEFAULT_MAX_ARCHIVE_BYTES, ExportResult, MarketplaceExportResult, PluginSyncService,
};
