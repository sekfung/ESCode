//! 命令来源描述表（`rust-u2-commands.md` §2/§4）。
use super::*;
use std::path::Path;

#[test]
fn the_only_legal_source_is_escode_agent() {
    assert_eq!(AGENT_SOURCES, ["escodeAgent"]);
    // 缺省与显式传 `escodeAgent` 同结果。
    let by_default = descriptor_for(None).unwrap();
    let explicit = descriptor_for(Some("escodeAgent")).unwrap();
    assert_eq!(by_default.agent_source, explicit.agent_source);
    assert_eq!(by_default.directory_source, "escode");
    // 未知来源报错（TS 在描述表查不到时读 `undefined.userDirectorySegments` 而抛错）。
    assert!(descriptor_for(Some("claudeCode")).is_err());
    assert!(descriptor_for(Some("")).is_err());
}

#[test]
fn user_directory_is_home_plus_the_descriptor_segments() {
    let descriptor = descriptor_for(None).unwrap();
    assert_eq!(
        descriptor.user_directory(Path::new("/home/me")),
        Path::new("/home/me/.escode/commands")
    );
    // 相对 home（测试与容器场景）也照样拼。
    assert_eq!(
        descriptor.user_directory(Path::new("relative")),
        Path::new("relative/.escode/commands")
    );
    // 描述表其余列与 TS 逐值一致（本片未用到，但漂移要能被发现）。
    assert_eq!(descriptor.user_directory_segments, [".escode", "commands"]);
    assert_eq!(
        descriptor.workspace_directory_segments,
        [".escode", "commands"]
    );
    assert_eq!(descriptor.file_extension, ".md");
    assert_eq!(descriptor.namespace_separator, "/");
    assert!(descriptor.supports_argument_hint);
}
