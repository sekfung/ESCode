//! skill / command / plugin 元数据（`rust-u2-sync.md` §3.4）。
use super::*;

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn skill_metadata_reads_yaml_with_loose_fallbacks() {
    let dir = tempfile::tempdir().unwrap();
    let skill = dir.path().join("alpha").join("SKILL.md");
    write(&skill, "---\nname: Alpha\nversion: 1.2.3\n---\nbody\n");
    let metadata = read_skill_metadata(&skill);
    assert_eq!(metadata.name, "Alpha");
    assert_eq!(metadata.version.as_deref(), Some("1.2.3"));

    // 数字版本按 String(number) 处理；无 frontmatter 回退目录名。
    write(&skill, "---\nname: 42\nversion: 7\n---\nbody\n");
    let metadata = read_skill_metadata(&skill);
    assert_eq!(metadata.name, "42", "非字符串 name 回退 loose/目录名");
    assert_eq!(metadata.version.as_deref(), Some("7"));

    let plain = dir.path().join("beta").join("SKILL.md");
    write(&plain, "no frontmatter");
    let metadata = read_skill_metadata(&plain);
    assert_eq!(metadata.name, "beta");
    assert!(metadata.version.is_none());
}

#[test]
fn broken_yaml_falls_back_to_loose_name_and_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let skill = dir.path().join("gamma").join("SKILL.md");
    write(
        &skill,
        "---\nbroken: [\nname: 'Loose Name'\nversion: \"9\"\n---\nbody\n",
    );
    let metadata = read_skill_metadata(&skill);
    assert_eq!(metadata.name, "Loose Name");
    assert_eq!(metadata.version.as_deref(), Some("9"));
}

#[test]
fn command_metadata_uses_the_relative_path_as_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("commands");
    let file = root.join("sub").join("run.md");
    write(
        &file,
        "---\ndescription: Run it\nargument-hint: <target>\n---\nbody\n",
    );
    let metadata = read_command_metadata(&file, &root);
    assert_eq!(metadata.name, "/sub/run");
    assert_eq!(metadata.description.as_deref(), Some("Run it"));
    assert_eq!(metadata.argument_hint.as_deref(), Some("<target>"));

    let missing = root.join("plain.md");
    let metadata = read_command_metadata(&missing, &root);
    assert_eq!(metadata.name, "/plain");
    assert!(metadata.description.is_none());
}

#[test]
fn plugin_metadata_prefers_the_escode_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join("plug");
    write(
        &plugin.join(".claude-plugin").join("plugin.json"),
        r#"{"name":"claude-name"}"#,
    );
    write(
        &plugin.join(".escode-plugin").join("plugin.json"),
        r#"{"name":"escode-name","version":" 2.0 "}"#,
    );
    let metadata = read_plugin_metadata(&plugin).unwrap();
    assert_eq!(metadata.name, "escode-name");
    assert_eq!(metadata.version.as_deref(), Some("2.0"));
    assert_eq!(metadata.id, "escode-name@inline");

    let bad = dir.path().join("bad");
    write(
        &bad.join(".escode-plugin").join("plugin.json"),
        r#"{"name":"  "}"#,
    );
    assert!(read_plugin_metadata(&bad).is_none());
    let broken = dir.path().join("broken");
    write(&broken.join(".escode-plugin").join("plugin.json"), "{oops");
    assert!(read_plugin_metadata(&broken).is_none());
}
