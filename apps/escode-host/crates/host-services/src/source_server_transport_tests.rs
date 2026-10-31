use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    BuildOptions, DockerTarget, ServerTarget, SshTarget, TransportSecrets, TransportTarget,
    WslTarget, build_posix_shell_exec_command, build_transport_command, parse_target,
    quote_posix_path_arg, quote_posix_shell_arg, resolve_posix_home_path,
};
use crate::source_server::ConnectionProfile;

fn profile(transport: &str, target: Value) -> ConnectionProfile {
    serde_json::from_value(json!({
        "profileId": "00000000-0000-4000-8000-000000000001",
        "transport": transport,
        "target": target,
        "createdAt": 1,
    }))
    .expect("valid profile")
}

fn args(command: &super::TransportCommand) -> Vec<String> {
    command
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn build(
    target: &TransportTarget,
    secrets: &TransportSecrets,
) -> anyhow::Result<super::TransportCommand> {
    build_transport_command(
        target,
        secrets,
        &BuildOptions {
            askpass_program: Path::new("/opt/escode/escode-host"),
            remote_command: &[
                "/home/dev/.escode/bin/escode".to_owned(),
                "serve".to_owned(),
            ],
            known_hosts: Some(Path::new("/data/.escode/v2/known_hosts")),
        },
    )
}

fn build_without_known_hosts(target: &TransportTarget) -> anyhow::Result<super::TransportCommand> {
    build_transport_command(
        target,
        &TransportSecrets::default(),
        &BuildOptions {
            askpass_program: Path::new("/opt/escode/escode-host"),
            remote_command: &["escode".to_owned(), "serve".to_owned()],
            known_hosts: None,
        },
    )
}

fn build_with_known_hosts(
    target: &TransportTarget,
    known_hosts: &Path,
) -> anyhow::Result<super::TransportCommand> {
    build_transport_command(
        target,
        &TransportSecrets::default(),
        &BuildOptions {
            askpass_program: Path::new("/opt/escode/escode-host"),
            remote_command: &["escode".to_owned(), "serve".to_owned()],
            known_hosts: Some(known_hosts),
        },
    )
}

/// 与 TS `quotePosixShellArg` / `quotePosixPathArg` / `resolvePosixHomePath` 逐字节一致。
#[test]
fn quotes_like_the_typescript_helpers() {
    assert_eq!(quote_posix_shell_arg(""), "''");
    assert_eq!(quote_posix_shell_arg("a b"), "'a b'");
    assert_eq!(quote_posix_shell_arg("it's"), r#"'it'"'"'s'"#);
    assert_eq!(quote_posix_shell_arg("--stdio"), "'--stdio'");
    assert_eq!(quote_posix_path_arg("~"), "\"$HOME\"");
    assert_eq!(quote_posix_path_arg("~/a b"), "\"$HOME\"'/a b'");
    assert_eq!(quote_posix_path_arg("/tmp/x"), "'/tmp/x'");
    assert_eq!(resolve_posix_home_path("~", "/home/dev"), "/home/dev");
    assert_eq!(resolve_posix_home_path("~/a", "/home/dev/"), "/home/dev/a");
    assert_eq!(resolve_posix_home_path("/srv/a", "/home/dev"), "/srv/a");
    assert_eq!(
        build_posix_shell_exec_command("echo hi"),
        "/bin/sh -c 'echo hi'"
    );
}

/// 密码只经 askpass env 传递；argv 里只出现目标与命令。
#[test]
fn ssh_password_uses_askpass_and_keeps_secret_out_of_argv() {
    let target = parse_target(&profile(
        "ssh",
        json!({
            "kind": "ssh",
            "host": "build.example",
            "port": 2222,
            "username": "dev",
            "passwordCredentialKey": "cred-1",
        }),
    ))
    .expect("ssh target");
    let secrets = TransportSecrets {
        password: Some("s3cr3t".to_owned()),
        ..TransportSecrets::default()
    };
    let command = build(&target, &secrets).expect("command");
    assert_eq!(command.program, PathBuf::from("ssh"));
    let args = args(&command);
    assert!(args.contains(&"-T".to_owned()));
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-o" && pair[1] == "BatchMode=no"),
        "密码认证不能 BatchMode=yes: {args:?}"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-o" && pair[1] == "StrictHostKeyChecking=accept-new")
    );
    assert!(args.windows(2).any(
        |pair| pair[0] == "-o" && pair[1] == "UserKnownHostsFile=/data/.escode/v2/known_hosts"
    ));
    assert!(
        !args.iter().any(|arg| arg == "/dev/null" || arg == "NUL"),
        "不得回退到 /dev/null known_hosts: {args:?}"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-p" && pair[1] == "2222")
    );
    assert_eq!(args[args.len() - 2], "dev@build.example");
    assert_eq!(
        args[args.len() - 1],
        "'/home/dev/.escode/bin/escode' 'serve'"
    );
    assert_eq!(command.env["SSH_ASKPASS"], "/opt/escode/escode-host");
    assert_eq!(command.env["SSH_ASKPASS_REQUIRE"], "force");
    assert_eq!(command.env["ESCODE_SSH_ASKPASS_SECRET"], "s3cr3t");
    assert!(
        !args.iter().any(|arg| arg.contains("s3cr3t")),
        "password 不得进入 argv: {args:?}"
    );
}

/// SSH 必须显式拿到 Desktop known_hosts；缺失时失败而不是静默关闭校验。
#[test]
fn ssh_requires_desktop_known_hosts() {
    let target = parse_target(&profile(
        "ssh",
        json!({"kind": "ssh", "host": "h", "username": "u"}),
    ))
    .expect("ssh target");
    let error = build_without_known_hosts(&target).expect_err("must fail");
    assert!(format!("{error:#}").contains("known_hosts"), "{error:#}");
}

/// macOS 默认数据目录含空格（`~/Library/Application Support`），Windows 路径含反斜杠；
/// OpenSSH 的 `-o` 参数取整段文本，因此这里必须原样透传，不做引号或转义改写（S4-i）。
#[test]
fn preserves_known_hosts_paths_for_macos_and_windows_data_dirs() {
    let target = parse_target(&profile(
        "ssh",
        json!({"kind": "ssh", "host": "h", "username": "u"}),
    ))
    .expect("ssh target");
    for path in [
        Path::new("/Users/dev/Library/Application Support/ESCode/.escode/v2/known_hosts"),
        Path::new(r"C:\Users\dev\AppData\Roaming\ESCode\.escode\v2\known_hosts"),
    ] {
        let command = build_with_known_hosts(&target, path).expect("command");
        let args = args(&command);
        let expected = format!("UserKnownHostsFile={}", path.to_string_lossy());
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-o" && pair[1] == expected),
            "known_hosts 路径必须原样进入 -o: {args:?}"
        );
    }
}

/// 仅识别 OpenSSH 明确的主机 key 变化诊断。
#[test]
fn classifies_only_explicit_host_key_changes() {
    assert!(super::is_host_key_changed_error(
        "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!"
    ));
    assert!(super::is_host_key_changed_error(
        "Host key verification failed."
    ));
    assert!(!super::is_host_key_changed_error(
        "Permission denied (publickey,password)."
    ));
    assert!(!super::is_host_key_changed_error(
        "remote transport closed before escode-hello"
    ));
}

/// 密钥认证用 BatchMode=yes 和 `-i`；没有 secret 时不注入 askpass。
#[test]
fn ssh_key_uses_batch_mode_and_identity_file() {
    let target = parse_target(&profile(
        "ssh",
        json!({
            "kind": "ssh",
            "host": "build.example",
            "username": "dev",
            "privateKeyPath": "/home/dev/.ssh/id_ed25519",
        }),
    ))
    .expect("ssh target");
    let command = build(&target, &TransportSecrets::default()).expect("command");
    let args = args(&command);
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-o" && pair[1] == "BatchMode=yes")
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "-i" && pair[1] == "/home/dev/.ssh/id_ed25519")
    );
    assert!(command.env.is_empty(), "密钥认证不应注入 askpass env");
}

/// profile 要求密码但 credential 没解出时显式失败，不静默回退 agent 认证。
#[test]
fn missing_password_secret_fails_explicitly() {
    let target = parse_target(&profile(
        "ssh",
        json!({
            "kind": "ssh",
            "host": "build.example",
            "username": "dev",
            "passwordCredentialKey": "cred-1",
        }),
    ))
    .expect("ssh target");
    let error = build(&target, &TransportSecrets::default()).expect_err("must fail");
    assert!(
        format!("{error:#}").contains("password not resolved"),
        "{error:#}"
    );
}

/// WSL / Docker 用 `sh -lc` 包住远端命令，参数顺序与 Node backend 一致。
#[test]
fn wsl_and_docker_wrap_the_remote_command() {
    let wsl = build(
        &TransportTarget::Wsl(WslTarget {
            distro: Some("Ubuntu-24.04".to_owned()),
            user: Some("dev".to_owned()),
        }),
        &TransportSecrets::default(),
    )
    .expect("wsl command");
    assert_eq!(wsl.program, PathBuf::from("wsl.exe"));
    assert_eq!(
        args(&wsl),
        vec![
            "-d",
            "Ubuntu-24.04",
            "-u",
            "dev",
            "--",
            "sh",
            "-lc",
            "'/home/dev/.escode/bin/escode' 'serve'",
        ]
    );

    let docker = build(
        &TransportTarget::Docker(DockerTarget {
            container: "escode-dev".to_owned(),
        }),
        &TransportSecrets::default(),
    )
    .expect("docker command");
    assert_eq!(docker.program, PathBuf::from("docker"));
    assert_eq!(
        args(&docker),
        vec![
            "exec",
            "-i",
            "escode-dev",
            "sh",
            "-lc",
            "'/home/dev/.escode/bin/escode' 'serve'",
        ]
    );
}

/// transport 判别式与必填字段：未知 transport / 缺字段显式报错。
#[test]
fn parses_each_transport_and_rejects_unknown() {
    assert_eq!(
        parse_target(&profile(
            "ssh",
            json!({"kind": "ssh", "host": "h", "username": "u"})
        ))
        .expect("ssh"),
        TransportTarget::Ssh(SshTarget {
            host: "h".to_owned(),
            port: 22,
            username: "u".to_owned(),
            private_key_path: None,
            password_credential_key: None,
            private_key_passphrase_credential_key: None,
        })
    );
    assert_eq!(
        parse_target(&profile("wsl", json!({"kind": "wsl"}))).expect("wsl"),
        TransportTarget::Wsl(WslTarget {
            distro: None,
            user: None,
        })
    );
    assert_eq!(
        parse_target(&profile(
            "docker",
            json!({"kind": "docker", "container": "c"})
        ))
        .expect("docker"),
        TransportTarget::Docker(DockerTarget {
            container: "c".to_owned(),
        })
    );
    assert_eq!(
        parse_target(&profile(
            "server",
            json!({"kind": "server", "url": "https://studio.example.com/escode/", "tokenCredentialKey": "connection-profile:p:token"})
        ))
        .expect("server"),
        TransportTarget::Server(ServerTarget {
            url: "https://studio.example.com/escode/".to_owned(),
            token_credential_key: Some("connection-profile:p:token".to_owned()),
        })
    );
    assert!(
        parse_target(&profile(
            "server",
            json!({"kind": "server", "tokenCredentialKey": "k"})
        ))
        .is_err(),
        "server 缺 url 必须失败"
    );
    assert!(
        parse_target(&profile("serial", json!({}))).is_err(),
        "未知 transport 必须失败"
    );
    assert!(
        parse_target(&profile("ssh", json!({"kind": "ssh", "username": "u"}))).is_err(),
        "缺 host 必须失败"
    );
}

/// server 目标没有本地进程；调用方误走命令构造时必须显式失败，而不是拿到空命令。
#[test]
fn server_transport_has_no_process_command() {
    let target = TransportTarget::Server(ServerTarget {
        url: "https://studio.example.com/escode/".to_owned(),
        token_credential_key: None,
    });
    let error = build(&target, &TransportSecrets::default()).expect_err("server command");
    assert!(
        format!("{error:#}").contains("server transport has no process command"),
        "{error:#}"
    );
}
