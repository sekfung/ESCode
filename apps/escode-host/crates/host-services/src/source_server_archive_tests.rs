use std::io::Read;
use std::path::PathBuf;

use flate2::read::GzDecoder;

use super::{ArchiveEntry, create_archive};
use crate::ustar::{self, BLOCK};

fn extract(archive: &[u8]) -> Vec<(String, Vec<u8>, u64)> {
    let mut decoder = GzDecoder::new(archive);
    let mut tar = Vec::new();
    decoder.read_to_end(&mut tar).expect("gunzip");
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + BLOCK <= tar.len() {
        let header: [u8; BLOCK] = tar[offset..offset + BLOCK].try_into().expect("header");
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let path = ustar::entry_path(&header);
        let size = ustar::parse_octal(&header[124..136]).expect("size");
        let mode = ustar::read_mode(&header);
        let data_start = offset + BLOCK;
        entries.push((
            path,
            tar[data_start..data_start + size as usize].to_vec(),
            mode,
        ));
        offset = data_start + ustar::padded(size).expect("padding") as usize;
    }
    entries
}

#[test]
fn archives_files_and_directories_with_release_layout() {
    let temp = tempfile::tempdir().expect("temp");
    std::fs::create_dir_all(temp.path().join("glm/linux-x64/packages")).expect("dir");
    std::fs::create_dir_all(temp.path().join("server")).expect("dir");
    std::fs::write(temp.path().join("glm/linux-x64/plugin-host.cjs"), b"bundle").expect("file");
    std::fs::write(temp.path().join("glm/linux-x64/packages/a.js"), b"plugin").expect("file");
    std::fs::write(temp.path().join("server/zcode-server.cjs"), b"server").expect("file");

    let archive = create_archive(&[
        ArchiveEntry {
            source_path: temp.path().join("glm/linux-x64"),
            archive_path: "glm/linux-x64".to_owned(),
        },
        ArchiveEntry {
            source_path: temp.path().join("server/zcode-server.cjs"),
            archive_path: "server/zcode-server.cjs".to_owned(),
        },
    ])
    .expect("archive");

    let entries = extract(&archive);
    let paths: Vec<&str> = entries.iter().map(|(path, _, _)| path.as_str()).collect();
    assert!(paths.contains(&"glm/linux-x64/"));
    assert!(paths.contains(&"glm/linux-x64/plugin-host.cjs"));
    assert!(paths.contains(&"glm/linux-x64/packages/a.js"));
    assert!(paths.contains(&"server/zcode-server.cjs"));
    let server = entries
        .iter()
        .find(|(path, _, _)| path == "server/zcode-server.cjs")
        .expect("server");
    assert_eq!(server.1, b"server");
}

#[test]
fn rejects_absolute_and_traversal_archive_paths() {
    let temp = tempfile::tempdir().expect("temp");
    let source = temp.path().join("file");
    std::fs::write(&source, b"x").expect("file");
    for bad in ["/abs", "../escape", "a/../b", "a\\\\b"] {
        assert!(
            create_archive(&[ArchiveEntry {
                source_path: source.clone(),
                archive_path: bad.to_owned(),
            }])
            .is_err(),
            "{bad}"
        );
    }
}

#[test]
fn missing_sources_are_reported() {
    let error = create_archive(&[ArchiveEntry {
        source_path: PathBuf::from("/definitely/missing/escode-asset"),
        archive_path: "node/linux-x64/node".to_owned(),
    }])
    .expect_err("missing");
    assert!(error.to_string().contains("read release asset"), "{error}");
}
