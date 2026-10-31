use std::cmp::Ordering;

use serde_json::json;

use super::{CompatibilityManifest, UpgradeTarget, compare_versions, evaluate};

fn manifest(product: &str, minimum: &str) -> CompatibilityManifest {
    CompatibilityManifest::new(product, minimum)
}

#[test]
fn compares_versions_like_the_typescript_implementation() {
    for (left, right, expected) in [
        ("3.15.2", "3.15.2", Ordering::Equal),
        ("v3.15.2", "3.15.2", Ordering::Equal),
        ("3.15.3", "3.15.2", Ordering::Greater),
        ("3.15.2", "3.15.3", Ordering::Less),
        ("3.16.0", "3.15.9", Ordering::Greater),
        ("3.15.2", "3.15.2-beta.1", Ordering::Greater),
        ("3.15.2-beta.2", "3.15.2-beta.10", Ordering::Less),
        ("3.15.2-beta", "3.15.2-alpha", Ordering::Greater),
        ("3.15.2-1", "3.15.2-alpha", Ordering::Less),
        ("3.15.2+build.1", "3.15.2", Ordering::Equal),
    ] {
        assert_eq!(
            compare_versions(left, right).expect("compare"),
            expected,
            "{left} vs {right}"
        );
    }
    assert!(compare_versions("3.15", "3.15.2").is_err());
    assert!(compare_versions("not-a-version", "3.15.2").is_err());
}

#[test]
fn compatible_newer_and_older_versions_map_to_update_availability() {
    let local = manifest("3.15.2", "3.15.0");

    let same = evaluate(&local, &manifest("3.15.2", "3.15.0")).expect("same");
    assert!(same.compatible);
    assert!(!same.update_available);
    assert_eq!(same.upgrade_target, None);

    let older_but_compatible = evaluate(&local, &manifest("3.15.0", "3.14.0")).expect("older");
    assert!(older_but_compatible.compatible);
    assert!(older_but_compatible.update_available);
    assert_eq!(older_but_compatible.upgrade_target, None);
    assert_eq!(older_but_compatible.newer_version, "3.15.2");
    assert_eq!(older_but_compatible.older_version, "3.15.0");
}

#[test]
fn incompatible_remote_requires_a_server_upgrade() {
    let local = manifest("3.15.2", "3.15.0");
    let result = evaluate(&local, &manifest("3.14.0", "3.13.0")).expect("older");
    assert!(!result.compatible);
    assert_eq!(result.upgrade_target, Some(UpgradeTarget::Server));
    assert_eq!(
        result.upgrade_target.map(UpgradeTarget::as_str),
        Some("server")
    );
    assert_eq!(result.required_minimum_version, "3.15.0");
}

#[test]
fn incompatible_newer_remote_requires_a_controller_upgrade() {
    let local = manifest("3.15.2", "3.15.0");
    let result = evaluate(&local, &manifest("3.16.0", "3.16.0")).expect("newer");
    assert!(!result.compatible);
    assert_eq!(result.upgrade_target, Some(UpgradeTarget::Controller));
    assert_eq!(
        result.upgrade_target.map(UpgradeTarget::as_str),
        Some("controller")
    );
    assert_eq!(result.required_minimum_version, "3.16.0");
}

#[test]
fn parses_strict_manifests_and_rejects_unknown_or_wrong_schema_versions() {
    let parsed = CompatibilityManifest::parse(&json!({
        "schemaVersion": 1,
        "productVersion": "3.15.2",
        "minCompatibleVersion": "3.15.2",
    }))
    .expect("manifest");
    assert_eq!(parsed, manifest("3.15.2", "3.15.2"));

    assert!(
        CompatibilityManifest::parse(&json!({
            "schemaVersion": 1,
            "productVersion": "3.15.2",
            "minCompatibleVersion": "3.15.2",
            "extra": true,
        }))
        .is_err()
    );
    assert!(
        CompatibilityManifest::parse(&json!({
            "schemaVersion": 2,
            "productVersion": "3.15.2",
            "minCompatibleVersion": "3.15.2",
        }))
        .is_err()
    );
}
