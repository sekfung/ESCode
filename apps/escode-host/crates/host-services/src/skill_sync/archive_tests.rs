//! 归档读写（`rust-u2-sync.md` §1.4）。
use super::*;
use crate::skill_sync::archive_tar;

fn compare() -> crate::skills::Compare {
    std::sync::Arc::new(|left: &str, right: &str| left.cmp(right))
}

fn write_file(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn archive_round_trips_directories_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("skill");
    write_file(&source.join("SKILL.md"), "# skill\n");
    write_file(&source.join("assets").join("b.txt"), "b");
    write_file(&source.join("assets").join("a.txt"), "a");

    let archive = create_archive(
        &[ArchiveEntry {
            source_path: source,
            archive_path: "group/skill".to_owned(),
        }],
        &compare(),
    )
    .unwrap();
    assert_eq!(&archive[..2], &[0x1f, 0x8b], "gzip 头");

    let target = dir.path().join("out");
    extract_archive(&archive, &target, Some(1024 * 1024)).unwrap();
    assert_eq!(
        std::fs::read_to_string(target.join("group").join("skill").join("SKILL.md")).unwrap(),
        "# skill\n"
    );
    assert_eq!(
        std::fs::read_to_string(
            target
                .join("group")
                .join("skill")
                .join("assets")
                .join("a.txt")
        )
        .unwrap(),
        "a"
    );

    // 目录内容按名字排序（a 在 b 之前）。
    let mut decoder = flate2::read::GzDecoder::new(archive.as_slice());
    let mut tar = Vec::new();
    decoder.read_to_end(&mut tar).unwrap();
    let a = tar
        .windows(b"assets/a.txt".len())
        .position(|window| window == b"assets/a.txt")
        .unwrap();
    let b = tar
        .windows(b"assets/b.txt".len())
        .position(|window| window == b"assets/b.txt")
        .unwrap();
    assert!(a < b);
}

// 修复：只有 Unix 上会创建软链（Windows 需要开发者模式），否则没有可拒绝的输入，限 Unix。
#[cfg(unix)]
#[test]
fn symlink_sources_are_rejected_like_the_typescript_archive() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("skill");
    std::fs::create_dir_all(&source).unwrap();
    write_file(&source.join("SKILL.md"), "x");
    std::os::unix::fs::symlink("/etc/hosts", source.join("link")).unwrap();
    let error = create_archive(
        &[ArchiveEntry {
            source_path: source,
            archive_path: "skill".to_owned(),
        }],
        &compare(),
    )
    .unwrap_err();
    assert!(
        error
            .message
            .starts_with("unsupported skill archive source:"),
        "{}",
        error.message
    );
}

fn gzip(tar: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(tar).unwrap();
    encoder.finish().unwrap()
}

fn crafted(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (path, flag, content) in entries {
        tar.extend_from_slice(
            &archive_tar::header(path, 0o644, 0, content.len() as u64, *flag).unwrap(),
        );
        tar.extend_from_slice(content);
        let padding = archive_tar::padded(content.len() as u64).unwrap() as usize - content.len();
        tar.extend(std::iter::repeat_n(0_u8, padding));
    }
    tar.extend_from_slice(&[0_u8; archive_tar::BLOCK * 2]);
    gzip(&tar)
}

#[test]
fn extraction_rejects_traversal_and_unknown_types() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let archive = crafted(&[("../escape.txt", b'0', b"x")]);
    let error = extract_archive(&archive, &target, Some(1024)).unwrap_err();
    assert_eq!(error.message, "unsafe skill archive path: ../escape.txt");
    assert!(!dir.path().join("escape.txt").exists());

    let archive = crafted(&[("link", b'2', b"")]);
    let error = extract_archive(&archive, &target, Some(1024)).unwrap_err();
    assert_eq!(error.message, "unsupported skill archive entry type: 2");
}

#[test]
fn extraction_enforces_the_byte_cap_and_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let archive = crafted(&[("skill/SKILL.md", b'0', b"0123456789")]);
    let error = extract_archive(&archive, dir.path(), Some(5)).unwrap_err();
    assert_eq!(
        error.message,
        "skill sync size limit exceeded: 10/5 (extracted-content)"
    );
    assert_eq!(
        error.code(),
        Some(crate::skill_sync::error::SIZE_LIMIT_CODE)
    );

    let mut truncated = Vec::new();
    let mut decoder = flate2::read::GzDecoder::new(archive.as_slice());
    decoder.read_to_end(&mut truncated).unwrap();
    truncated.truncate(515); // 头 + 前 3 字节数据
    let error = extract_archive(&gzip(&truncated), dir.path(), Some(1024)).unwrap_err();
    assert_eq!(error.message, "truncated skill sync archive entry");
}
