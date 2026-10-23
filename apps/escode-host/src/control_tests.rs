//! `WorkspaceDescriptor` → `RuntimeOptions` 的 env 合成（审计 A16）。

use super::*;

fn workspace(identity: Option<&str>) -> WorkspaceDescriptor {
    WorkspaceDescriptor {
        path: "/w".to_owned(),
        identity: identity.map(str::to_owned),
        ..WorkspaceDescriptor::default()
    }
}

#[test]
fn runtime_env_from_main_reaches_the_instance_snapshot() {
    let runtime_env = BTreeMap::from([
        (
            "ESCODE_BUILTIN_PROVIDER_CONFIG_FILE".to_owned(),
            "/cfg/builtin.json".to_owned(),
        ),
        (
            "ESCODE_PERSONAL_PROVIDER_CONFIG_FILE".to_owned(),
            "/cfg/personal.json".to_owned(),
        ),
        (
            "ESCODE_WORKSPACE_IDENTITY".to_owned(),
            "from-main".to_owned(),
        ),
    ]);
    let options = runtime_options_with(&workspace(Some("remote:abc")), &runtime_env);
    assert_eq!(options.legacy_import, LegacyImport::Implicit);
    assert_eq!(
        options.env["ESCODE_BUILTIN_PROVIDER_CONFIG_FILE"],
        "/cfg/builtin.json"
    );
    assert_eq!(
        options.env["ESCODE_PERSONAL_PROVIDER_CONFIG_FILE"],
        "/cfg/personal.json"
    );
    assert_eq!(
        options.env["ESCODE_WORKSPACE_IDENTITY"], "remote:abc",
        "workspace 身份由描述决定，不被 Main 的同名项覆盖"
    );
    let without = runtime_options_with(&workspace(None), &runtime_env);
    assert_eq!(
        without.env["ESCODE_WORKSPACE_IDENTITY"], "from-main",
        "无身份时不擅自删除 Main 提供的项"
    );
}

#[test]
fn window_patch_overrides_host_env_and_identity_wins() {
    let base = BTreeMap::from([
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        (
            "ESCODE_WORKSPACE_IDENTITY".to_owned(),
            "from-main".to_owned(),
        ),
    ]);
    let patch = BTreeMap::from([
        ("PATH".to_owned(), "/window/bin:/usr/bin:/bin".to_owned()),
        ("ESCODE_WINDOW_PATCH".to_owned(), "1".to_owned()),
    ]);
    let options = runtime_options_with_patch(&workspace(Some("id")), &base, Some(&patch));
    assert_eq!(options.env["PATH"], "/window/bin:/usr/bin:/bin");
    assert_eq!(options.env["ESCODE_WINDOW_PATCH"], "1");
    assert_eq!(options.env["ESCODE_WORKSPACE_IDENTITY"], "id");

    let without_patch = runtime_options_with_patch(&workspace(Some("id")), &base, None);
    assert_eq!(without_patch.env["PATH"], "/usr/bin:/bin");
    assert!(!without_patch.env.contains_key("ESCODE_WINDOW_PATCH"));
}

#[test]
fn legacy_entry_adds_only_the_workspace_identity() {
    let options = runtime_options(&workspace(Some("id")));
    assert_eq!(options.legacy_import, LegacyImport::Implicit);
    assert_eq!(options.env.len(), 1);
    assert_eq!(options.env["ESCODE_WORKSPACE_IDENTITY"], "id");
}
