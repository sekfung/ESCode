use serde_json::{Value, json};

use super::{
    ComponentMeta, DeployDecisionOptions, build_install_preview, check_component_identity,
    component_cache_version, expected_mount, normalize_relative_path, parse_component_meta,
    parse_release_manifest, should_deploy_component,
};

fn manifest_json() -> Value {
    json!({
        "schemaVersion": 1,
        "appVersion": "3.15.2",
        "platformArch": "linux-x64",
        "components": [
            {
                "id": "server-bundle",
                "version": "v3.15.2+11634cd500db",
                "sha256": "11634cd500dbc2b441c4908a2fa29b2ce265a9972861fb69691bbca16f520867",
                "sourceSha256": "ignored",
                "artifactPath": "components/linux-x64/server-bundle/v3.15.2+11634cd500db.tar.gz",
                "mount": "server"
            },
            {
                "id": "node-runtime",
                "version": "v22.16.0+09fafeec368a",
                "sha256": "09Fafeec368a2b4f3d6615c9b5932da650354ce578b9179639dc7619b32eab67",
                "artifactPath": "components/linux-x64/node-runtime/v22.16.0+09fafeec368a.tar.gz",
                "mount": "node/linux-x64"
            },
            {
                "id": "retired-agent",
                "version": "v0.0.1",
                "sha256": "not-validated-for-unknown-id",
                "artifactPath": "retired.tar.gz",
                "mount": "retired"
            }
        ]
    })
}

fn parsed() -> super::ReleaseManifest {
    parse_release_manifest(&manifest_json(), "3.15.2", "linux-x64").expect("manifest")
}

fn meta(version: &str, sha256: Option<&str>) -> ComponentMeta {
    ComponentMeta {
        id: "server-bundle".to_owned(),
        version: Some(version.to_owned()),
        sha256: sha256.map(str::to_owned),
        pending_refresh_app_version: None,
        platform_arch: "linux-x64".to_owned(),
    }
}

fn decision(
    remote_exists: bool,
    expected_version: Option<&str>,
    force: bool,
    fallback: bool,
    remote_meta: Option<&ComponentMeta>,
) -> super::DeployDecision {
    should_deploy_component(DeployDecisionOptions {
        component_id: "server-bundle",
        platform_arch: "linux-x64",
        remote_path: "~/.escode/server/zcode-server.cjs",
        remote_exists,
        expected_version,
        force,
        fallback_deploy_when_version_unknown: fallback,
        remote_meta,
    })
}

#[test]
fn parses_known_components_and_skips_unknown_ids() {
    let manifest = parsed();
    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.components.len(), 2);
    assert_eq!(manifest.components[0].id, "server-bundle");
    assert_eq!(manifest.components[0].mount, "server");
    assert_eq!(manifest.components[0].cache_version(), "v3.15.2");
    assert!(manifest.components[0].uses_content_addressed_identity());
    assert_eq!(manifest.components[1].mount, "node/linux-x64");
    assert!(!manifest.components[1].uses_content_addressed_identity());
    // sha256 按 TS 规则转小写。
    assert!(
        manifest.components[1]
            .sha256
            .chars()
            .all(|value| !value.is_ascii_uppercase())
    );
}

#[test]
fn rejects_manifest_identity_and_component_shape_errors() {
    let mut value = manifest_json();
    value["schemaVersion"] = json!(2);
    assert!(parse_release_manifest(&value, "3.15.2", "linux-x64").is_err());
    value["schemaVersion"] = json!(1.5);
    assert!(parse_release_manifest(&value, "3.15.2", "linux-x64").is_err());
    value["schemaVersion"] = json!(1);
    assert!(parse_release_manifest(&value, "3.15.3", "linux-x64").is_err());
    assert!(parse_release_manifest(&value, "3.15.2", "linux-arm64").is_err());

    value["components"] = json!([]);
    assert!(parse_release_manifest(&value, "3.15.2", "linux-x64").is_err());

    for (path, bad) in [
        ("id", json!("Server_Bundle")),
        ("version", json!("v1/../../escape")),
        ("sha256", json!("abc")),
        ("artifactPath", json!("/abs/path")),
        ("artifactPath", json!("C:/abs/path")),
        ("artifactPath", json!("a//b")),
        ("artifactPath", json!("a/../b")),
        ("artifactPath", json!("a\\b")),
        ("mount", json!("server/")),
        ("mount", json!("other")),
    ] {
        let mut value = manifest_json();
        value["components"][0][path] = bad.clone();
        assert!(
            parse_release_manifest(&value, "3.15.2", "linux-x64").is_err(),
            "{path} should reject {bad}"
        );
    }

    let mut value = manifest_json();
    value["components"][1]["id"] = json!("server-bundle");
    assert!(parse_release_manifest(&value, "3.15.2", "linux-x64").is_err());
    value["components"][1]["id"] = json!("retired-agent");
    assert!(parse_release_manifest(&value, "3.15.2", "linux-x64").is_err());
}

#[test]
fn softens_versions_only_for_hash_suffixes() {
    for (version, expected) in [
        ("v3.15.2+11634cd500db", "v3.15.2"),
        ("v1+ABCDEF123456", "v1"),
        ("v1+abc", "v1+abc"),
        ("v1+zzzzzzzzzzzz", "v1+zzzzzzzzzzzz"),
        ("v1+", "v1+"),
        (
            "v1+11111111111111111111111111111111111111111111111111111111111111111",
            "v1+11111111111111111111111111111111111111111111111111111111111111111",
        ),
        ("v1", "v1"),
    ] {
        assert_eq!(component_cache_version(version), expected, "{version}");
    }
}

#[test]
fn path_helpers_match_typescript_rules() {
    assert_eq!(normalize_relative_path(" a/b ", "path").unwrap(), "a/b");
    assert_eq!(
        expected_mount("bfs", "linux-x64").unwrap(),
        "tools/linux-x64/bfs"
    );
    assert!(expected_mount("unknown", "linux-x64").is_none());
    assert!(normalize_relative_path("", "path").is_err());
    assert!(normalize_relative_path("a/./b", "path").is_err());
}

#[test]
fn deploy_decision_mirrors_typescript_branch_order() {
    let expected = "v3.15.2+11634cd500db";
    let matching = meta(expected, Some("abc"));
    assert!(!decision(true, Some(expected), false, true, Some(&matching)).should_deploy);
    assert_eq!(
        decision(true, Some(expected), true, true, Some(&matching))
            .reason
            .as_deref(),
        Some("force deploy requested")
    );
    assert_eq!(
        decision(false, Some(expected), false, true, Some(&matching))
            .reason
            .as_deref(),
        Some("remote file missing path=~/.escode/server/zcode-server.cjs")
    );
    assert!(decision(true, None, false, true, Some(&matching)).should_deploy);
    assert!(!decision(true, None, false, false, Some(&matching)).should_deploy);
    assert!(decision(true, Some(expected), false, true, None).should_deploy);

    let mut wrong_id = matching.clone();
    wrong_id.id = "glm".to_owned();
    assert!(decision(true, Some(expected), false, true, Some(&wrong_id)).should_deploy);
    let wrong_platform = ComponentMeta {
        platform_arch: "darwin-arm64".to_owned(),
        ..matching
    };
    assert!(decision(true, Some(expected), false, true, Some(&wrong_platform)).should_deploy);

    // 远端 meta 存的是带 hash 版本或已剥离版本，都应视为同一缓存版本。
    let stripped = meta("v3.15.2", Some("abc"));
    assert!(!decision(true, Some(expected), false, true, Some(&stripped)).should_deploy);
    let other = meta("v3.15.1", Some("abc"));
    assert!(decision(true, Some(expected), false, true, Some(&other)).should_deploy);
}

#[test]
fn identity_decision_requires_the_manifest_sha() {
    let same = meta("v3.15.2", Some("ABCdef"));
    assert!(
        !check_component_identity(Some(&same), "server-bundle", "linux-x64", "abcdef")
            .should_deploy
    );
    for remote in [
        None,
        Some(meta("v3.15.2", None)),
        Some(meta("v3.15.2", Some("other"))),
    ] {
        assert!(
            check_component_identity(remote.as_ref(), "server-bundle", "linux-x64", "abcdef")
                .should_deploy
        );
    }
    assert!(
        check_component_identity(
            Some(&meta("v3.15.2", Some("abcdef"))),
            "glm",
            "linux-x64",
            "abcdef"
        )
        .should_deploy
    );
    assert!(
        check_component_identity(
            Some(&meta("v3.15.2", Some("abcdef"))),
            "server-bundle",
            "darwin-arm64",
            "abcdef"
        )
        .should_deploy
    );
}

#[test]
fn component_meta_round_trips_with_node_field_order() {
    let parsed = parse_component_meta(
        r#"{"id":"node-runtime","version":"v22.16.0","sha256":"ab","platformArch":"linux-x64"}"#,
    )
    .expect("meta");
    assert_eq!(parsed.version.as_deref(), Some("v22.16.0"));
    assert_eq!(
        parsed.to_json_line().expect("json"),
        "{\"id\":\"node-runtime\",\"version\":\"v22.16.0\",\"sha256\":\"ab\",\"platformArch\":\"linux-x64\"}\n"
    );
    assert!(parse_component_meta("not json").is_none());
    assert!(parse_component_meta(r#"{"id":"x","platformArch":null}"#).is_none());
    assert!(parse_component_meta(r#"{"id":"x","version":1,"platformArch":"linux-x64"}"#).is_none());
    assert!(parse_component_meta(r#"{"platformArch":"linux-x64"}"#).is_none());
}

#[test]
fn install_preview_uses_app_version_and_remote_home() {
    let manifest = parsed();
    let preview = build_install_preview(&manifest, Some("/home/dev"), 1_048_576).expect("preview");
    assert_eq!(preview.version, "3.15.2");
    assert_eq!(
        preview.install_path.as_deref(),
        Some("/home/dev/.escode/server")
    );
    assert_eq!(preview.size_bytes, 1_048_576);
    let without_home = build_install_preview(&manifest, None, 1).expect("preview");
    assert!(without_home.install_path.is_none());
    assert!(build_install_preview(&manifest, Some("/home/dev"), 0).is_err());
}
