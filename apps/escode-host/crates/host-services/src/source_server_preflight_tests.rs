use super::{
    DownloadTool, Sha256Tool, normalize_remote_arch, normalize_remote_platform, parse_probe,
    parse_tools, probe_script, resolve_remote_platform,
};

fn tools_output(download: &str, tar: &str, sha256: &str) -> String {
    format!("download={download}\ntar={tar}\nsha256={sha256}\n")
}

#[test]
fn parses_tool_candidates_in_priority_order() {
    let curl = parse_tools(&tools_output("curl", "tar", "sha256sum")).expect("curl");
    assert_eq!(curl.download, DownloadTool::Curl);
    assert!(curl.tar);
    assert_eq!(curl.sha256, Sha256Tool::Sha256sum);

    let wget = parse_tools(&tools_output("wget", "tar", "shasum")).expect("wget");
    assert_eq!(wget.download, DownloadTool::Wget);
    assert_eq!(wget.sha256, Sha256Tool::Shasum);
    assert_eq!(
        parse_tools(&tools_output("curl", "tar", "openssl"))
            .expect("openssl")
            .sha256,
        Sha256Tool::Openssl
    );
}

#[test]
fn rejects_missing_remote_download_tools() {
    let missing_download =
        parse_tools(&tools_output("", "tar", "sha256sum")).expect_err("download");
    assert!(missing_download.to_string().contains("curl or wget"));
    let missing_tar = parse_tools(&tools_output("curl", "", "sha256sum")).expect_err("tar");
    assert!(missing_tar.to_string().contains("missing tar"));
    let missing_sha = parse_tools(&tools_output("curl", "tar", "")).expect_err("sha");
    assert!(missing_sha.to_string().contains("sha256sum"));
    assert!(parse_tools(&tools_output("aria2c", "tar", "sha256sum")).is_err());
    assert!(parse_tools(&tools_output("curl", "bsdtar", "md5")).is_err());
}

#[test]
fn normalizes_remote_platform_and_arch_like_typescript() {
    for (raw, expected) in [
        ("Darwin", "darwin"),
        ("macOS", "darwin"),
        ("Linux", "linux"),
        ("GNU/Linux", "linux"),
        ("Windows_NT", "win32"),
        ("MINGW64_NT-10.0", "win32"),
        ("MSYS_NT-10.0", "win32"),
        ("CYGWIN_NT-10.0", "win32"),
        ("FreeBSD", "freebsd"),
    ] {
        assert_eq!(normalize_remote_platform(raw), expected, "{raw}");
    }
    for (raw, expected) in [
        ("x86_64", "x64"),
        ("AMD64", "x64"),
        ("aarch64", "arm64"),
        ("arm64e", "arm64"),
        ("riscv64", "riscv64"),
    ] {
        assert_eq!(normalize_remote_arch(raw), expected, "{raw}");
    }
    assert_eq!(resolve_remote_platform("Darwin", "Linux"), "linux");
    assert_eq!(resolve_remote_platform("Darwin", ""), "darwin");
    assert_eq!(resolve_remote_platform("Linux", "Linux"), "linux");
}

#[test]
fn parses_a_complete_probe_payload() {
    let stdout = format!(
        "platform=Linux\narch=x86_64\nkernel_ostype=Linux\nhome=/root\n{}",
        tools_output("curl", "tar", "sha256sum")
    );
    let probe = parse_probe(&stdout).expect("probe");
    assert_eq!(probe.environment.platform, "linux");
    assert_eq!(probe.environment.arch, "x64");
    assert_eq!(probe.environment.platform_arch, "linux-x64");
    assert_eq!(probe.environment.home.as_deref(), Some("/root"));
    assert_eq!(probe.tools.download, DownloadTool::Curl);

    let without_home = format!(
        "platform=Linux\narch=aarch64\nkernel_ostype=Linux\nhome=\n{}",
        tools_output("wget", "tar", "shasum")
    );
    let probe = parse_probe(&without_home).expect("probe");
    assert_eq!(probe.environment.platform_arch, "linux-arm64");
    assert!(probe.environment.home.is_none());
}

#[test]
fn probe_payload_requires_platform_and_arch() {
    let missing_platform = format!("arch=x86_64\n{}", tools_output("curl", "tar", "sha256sum"));
    assert!(parse_probe(&missing_platform).is_err());
    let missing_arch = format!(
        "platform=Linux\n{}",
        tools_output("curl", "tar", "sha256sum")
    );
    assert!(parse_probe(&missing_arch).is_err());
}

#[test]
fn probe_script_combines_environment_and_tool_detection() {
    let script = probe_script();
    assert!(script.contains("uname -s"));
    assert!(script.contains("uname -m"));
    assert!(script.contains("/proc/sys/kernel/ostype"));
    assert!(script.contains("command -v curl"));
    assert!(script.contains("command -v tar"));
    assert!(script.contains("command -v sha256sum"));
    // 一次往返完成探测，避免每个字段单独 SSH。
    assert_eq!(script.matches("uname -s").count(), 1);
}
