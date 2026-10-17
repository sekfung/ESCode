//! 候选扫描（`rust-u2-sync.md` §1.2）。
use super::*;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn compare() -> Compare {
    std::sync::Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn frontmatter(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\nbody\n")
}

#[test]
fn collects_escode_first_and_filters_covered_agents_candidates() {
    let home = tempfile::tempdir().unwrap();
    let escode = home.path().join(".escode/skills");
    let agents = home.path().join(".agents/skills");
    write(
        &escode.join("alpha/SKILL.md"),
        &frontmatter("Alpha", " first "),
    );
    write(
        &escode.join("group/beta/SKILL.md"),
        &frontmatter("beta", ""),
    );
    // 目录名被 .escode 覆盖（即使 skill 名不同也过滤）。
    write(&agents.join("alpha/SKILL.md"), &frontmatter("Other", ""));
    // skill 名与 .escode 重名（trim + lowercase）。
    write(
        &agents.join("renamed/SKILL.md"),
        &frontmatter(" alpha ", ""),
    );
    write(
        &agents.join("gamma/SKILL.md"),
        &frontmatter("gamma", "third"),
    );

    let candidates = collect(home.path(), &compare()).unwrap();
    let entries: Vec<(&str, &str)> = candidates
        .iter()
        .map(|candidate| (candidate.directory_name.as_str(), candidate.name.as_str()))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("alpha", "Alpha"),
            ("gamma", "gamma"),
            ("group/beta", "beta")
        ]
    );
    assert_eq!(candidates[0].description, "first");
    assert_eq!(
        candidates[0].id,
        format!(
            "{:x}",
            Sha256::digest(
                candidates[0]
                    .path
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes()
            )
        )
    );
    assert!(candidates[0].size_bytes > 0);

    let by_name = directory_path_by_name(home.path()).unwrap();
    assert_eq!(
        by_name.get("alpha").unwrap(),
        &escode.join("alpha"),
        ".escode 优先"
    );
    assert_eq!(by_name.get("gamma").unwrap(), &agents.join("gamma"));
}

#[test]
fn falls_back_to_the_directory_name_and_respects_the_scan_policy() {
    let home = tempfile::tempdir().unwrap();
    let escode = home.path().join(".escode/skills");
    write(&escode.join("plain/SKILL.md"), "no frontmatter\n");
    write(
        &escode.join("broken/SKILL.md"),
        "---\nname: [unclosed\n---\nbody\n",
    );
    write(
        &escode.join("node_modules/pkg/SKILL.md"),
        &frontmatter("pkg", ""),
    );
    write(&escode.join(".hidden/SKILL.md"), &frontmatter("hidden", ""));
    write(&escode.join("SKILL.md"), &frontmatter("root", ""));

    let candidates = collect(home.path(), &compare()).unwrap();
    let names: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["broken", "plain"],
        "根目录自身与排除目录不算候选"
    );
}

#[cfg(unix)]
#[test]
fn dedupes_symlinked_candidates_by_realpath() {
    let home = tempfile::tempdir().unwrap();
    let escode = home.path().join(".escode/skills");
    let agents = home.path().join(".agents/skills");
    write(&escode.join("alpha/SKILL.md"), &frontmatter("alpha", ""));
    std::fs::create_dir_all(&agents).unwrap();
    std::os::unix::fs::symlink(escode.join("alpha"), agents.join("linked")).unwrap();

    let candidates = collect(home.path(), &compare()).unwrap();
    assert_eq!(candidates.len(), 1, "同一 SKILL.md 只出现一次");
    assert_eq!(candidates[0].directory_name, "alpha");

    // 目录体积跟随软链目标。
    write(&home.path().join("payload.bin"), &"x".repeat(64));
    std::os::unix::fs::symlink(
        home.path().join("payload.bin"),
        escode.join("alpha/payload-link"),
    )
    .unwrap();
    let candidates = collect(home.path(), &compare()).unwrap();
    assert!(candidates[0].size_bytes >= 64);
}
