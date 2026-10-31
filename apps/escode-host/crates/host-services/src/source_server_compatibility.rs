//! Source Server 兼容清单与版本判定（S4-a，docs/specs/source-server-remote-rust-host.md §11.4）。
//!
//! 与 TS `packages/shared/src/source-server/compatibility.ts` 对等：解析远端 hello 的可选
//! `compatibility` 字段，按 `minCompatibleVersion` 判定「兼容 / 需升级 Server / 需升级 Desktop」。
//! 版本解析与比较规则（含 prerelease）逐项对齐 TS `compareCompatibilityVersions`。

use std::cmp::Ordering;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const COMPATIBILITY_SCHEMA_VERSION: u8 = 1;

/// 远端 `escode-hello.compatibility` 与本地产品清单（TS `compatibilityManifestSchema`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompatibilityManifest {
    pub schema_version: u8,
    pub product_version: String,
    pub min_compatible_version: String,
}

impl CompatibilityManifest {
    pub fn new(
        product_version: impl Into<String>,
        min_compatible_version: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: COMPATIBILITY_SCHEMA_VERSION,
            product_version: product_version.into(),
            min_compatible_version: min_compatible_version.into(),
        }
    }

    /// 本地默认清单：`minCompatibleVersion` 缺省等于本机产品版本（与 TS 构建常量一致）。
    pub fn local(product_version: impl Into<String>) -> Self {
        let product_version = product_version.into();
        Self::new(product_version.clone(), product_version)
    }

    /// 严格解析；`schemaVersion` 不是 1 或字段缺失时显式报错。
    pub fn parse(value: &Value) -> Result<Self> {
        let manifest: Self = serde_json::from_value(value.clone())
            .map_err(|error| anyhow::anyhow!("invalid compatibility manifest: {error}"))?;
        if manifest.schema_version != COMPATIBILITY_SCHEMA_VERSION {
            bail!(
                "unsupported compatibility manifest schemaVersion: {}",
                manifest.schema_version
            );
        }
        if manifest.product_version.trim().is_empty()
            || manifest.min_compatible_version.trim().is_empty()
        {
            bail!("compatibility manifest versions must not be empty");
        }
        Ok(manifest)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpgradeTarget {
    Controller,
    Server,
}

impl UpgradeTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            UpgradeTarget::Controller => "controller",
            UpgradeTarget::Server => "server",
        }
    }
}

/// 兼容判定结果（TS `SourceServerCompatibilityResult` 的子集）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatibilityEvaluation {
    pub compatible: bool,
    pub update_available: bool,
    pub upgrade_target: Option<UpgradeTarget>,
    pub newer_version: String,
    pub older_version: String,
    pub required_minimum_version: String,
}

/// 与 TS `evaluateSourceServerCompatibility` 同一判定顺序。
pub fn evaluate(
    controller: &CompatibilityManifest,
    server: &CompatibilityManifest,
) -> Result<CompatibilityEvaluation> {
    let comparison = compare_versions(&controller.product_version, &server.product_version)?;
    let (newer, older) = match comparison {
        Ordering::Less => (server, controller),
        Ordering::Equal | Ordering::Greater => (controller, server),
    };
    let newer_is_controller = std::ptr::eq(newer, controller);
    let compatible =
        compare_versions(&older.product_version, &newer.min_compatible_version)? != Ordering::Less;
    Ok(CompatibilityEvaluation {
        compatible,
        update_available: comparison != Ordering::Equal,
        upgrade_target: if compatible {
            None
        } else {
            Some(if newer_is_controller {
                UpgradeTarget::Server
            } else {
                UpgradeTarget::Controller
            })
        },
        newer_version: newer.product_version.clone(),
        older_version: older.product_version.clone(),
        required_minimum_version: newer.min_compatible_version.clone(),
    })
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedVersion {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Vec<String>,
}

fn parse_version(value: &str) -> Result<ParsedVersion> {
    let trimmed = value.trim();
    let without_v = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let without_build = without_v
        .split_once('+')
        .map_or(without_v, |(head, _)| head);
    let (core, prerelease) = match without_build.split_once('-') {
        Some((core, prerelease)) if !prerelease.is_empty() => {
            let identifiers = prerelease.split('.').map(str::to_owned).collect::<Vec<_>>();
            if identifiers.iter().any(String::is_empty) {
                bail!("Invalid compatibility version: {value}");
            }
            (core, identifiers)
        }
        Some(_) => bail!("Invalid compatibility version: {value}"),
        None => (without_build, Vec::new()),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        bail!("Invalid compatibility version: {value}");
    }
    let number = |part: &str| -> Result<u64> {
        part.parse::<u64>()
            .map_err(|_| anyhow::anyhow!("Invalid compatibility version: {value}"))
    };
    Ok(ParsedVersion {
        major: number(parts[0])?,
        minor: number(parts[1])?,
        patch: number(parts[2])?,
        prerelease,
    })
}

/// 与 TS `compareCompatibilityVersions` 一致：正式版大于 prerelease；数字 identifier 小于字母。
pub fn compare_versions(left: &str, right: &str) -> Result<Ordering> {
    let a = parse_version(left)?;
    let b = parse_version(right)?;
    for (left, right) in [(a.major, b.major), (a.minor, b.minor), (a.patch, b.patch)] {
        if left != right {
            return Ok(left.cmp(&right));
        }
    }
    match (a.prerelease.is_empty(), b.prerelease.is_empty()) {
        (true, true) => Ok(Ordering::Equal),
        (true, false) => Ok(Ordering::Greater),
        (false, true) => Ok(Ordering::Less),
        (false, false) => compare_prerelease(&a.prerelease, &b.prerelease),
    }
}

fn compare_prerelease(left: &[String], right: &[String]) -> Result<Ordering> {
    let length = left.len().max(right.len());
    for index in 0..length {
        match (left.get(index), right.get(index)) {
            (None, None) => return Ok(Ordering::Equal),
            (None, Some(_)) => return Ok(Ordering::Less),
            (Some(_), None) => return Ok(Ordering::Greater),
            (Some(left), Some(right)) => match compare_identifier(left, right) {
                Ordering::Equal => continue,
                ordering => return Ok(ordering),
            },
        }
    }
    Ok(Ordering::Equal)
}

fn compare_identifier(left: &str, right: &str) -> Ordering {
    let left_numeric = left.bytes().all(|byte| byte.is_ascii_digit());
    let right_numeric = right.bytes().all(|byte| byte.is_ascii_digit());
    match (left_numeric, right_numeric) {
        (true, true) => left
            .parse::<u64>()
            .unwrap_or(u64::MAX)
            .cmp(&right.parse::<u64>().unwrap_or(u64::MAX)),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => left.cmp(right),
    }
}

#[cfg(test)]
#[path = "source_server_compatibility_tests.rs"]
mod tests;
