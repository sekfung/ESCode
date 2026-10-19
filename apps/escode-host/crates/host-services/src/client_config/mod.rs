//! `client-config`（docs/specs/rust-u2-client-config.md；TS `client-config/clientConfigService.ts`）。

mod dynamic_service;
pub mod dynamic_workflow;
mod off_peak;
mod service;
mod snapshot;

pub use service::{ClientConfigService, ConfigError, parse_read_options};
pub use snapshot::{ClientConfigSnapshot, parse_snapshot};

#[cfg(test)]
#[path = "dynamic_tests.rs"]
mod dynamic_tests;
#[cfg(test)]
#[path = "service_tests.rs"]
mod service_tests;
#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod snapshot_tests;
