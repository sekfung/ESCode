//! S4-j identity probe 单测。

use super::*;

fn output(stdout: &str) -> CommandOutput {
    CommandOutput {
        stdout: stdout.to_owned(),
        stderr: String::new(),
        exit_code: Some(0),
    }
}

#[test]
fn parses_present_marker_with_identity_fields() {
    let raw = "present\0{\"installationId\":\"11111111-1111-4111-8111-111111111111\"}\0dev\x00host-1\0/srv/work\0";
    let parsed = parse_read_output(&output(raw)).expect("parse");
    assert!(matches!(parsed.marker, Marker::Present(_)));
    assert_eq!(parsed.os_user.as_deref(), Some("dev"));
    assert_eq!(parsed.hostname.as_deref(), Some("host-1"));
    assert_eq!(parsed.workspace_path.as_deref(), Some("/srv/work"));
}

#[test]
fn parses_missing_marker_with_canonical_root() {
    let raw = "missing\0/data/home/.escode/server\0\x00\x00\0";
    let parsed = parse_read_output(&output(raw)).expect("parse");
    match parsed.marker {
        Marker::Missing { canonical_root } => {
            assert_eq!(canonical_root, "/data/home/.escode/server")
        }
        Marker::Present(_) => panic!("expected missing marker"),
    }
    assert!(parsed.os_user.is_none());
    assert!(parsed.workspace_path.is_none());
}

#[test]
fn rejects_unknown_status_and_missing_fields() {
    assert!(parse_read_output(&output("other\0x\0a\0b\0c\0")).is_err());
    assert!(parse_read_output(&output("present\0{}\0a\0b\0")).is_err());
}

#[test]
fn rejects_non_uuid_installation_id() {
    let error = identity_from_marker(r#"{"installationId":"not-a-uuid"}"#, None, None, None)
        .expect_err("invalid uuid");
    assert!(format!("{error:#}").contains("not a UUID"), "{error:#}");
    assert!(identity_from_marker("{}", None, None, None).is_err());
    assert!(identity_from_marker("not-json", None, None, None).is_err());
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::source_server_exec::run_command;
    use crate::source_server_transport::TransportCommand;
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tokio::process::Child;

    struct ShRunner {
        home: PathBuf,
    }

    #[async_trait]
    impl RemoteRunner for ShRunner {
        async fn run(
            &self,
            script: &str,
            stdin: Option<&[u8]>,
            timeout: Duration,
        ) -> Result<CommandOutput> {
            let command = TransportCommand {
                program: "sh".into(),
                args: vec!["-c".into(), script.to_owned().into()],
                env: BTreeMap::from([(
                    "HOME".to_owned(),
                    self.home.to_string_lossy().into_owned(),
                )]),
            };
            run_command(&command, stdin, timeout).await
        }

        fn spawn_piped(&self, _script: &str) -> Result<Child> {
            bail!("not used")
        }
    }

    #[tokio::test]
    async fn creates_marker_once_and_reuses_the_same_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runner: Arc<dyn RemoteRunner> = Arc::new(ShRunner {
            home: temp.path().to_path_buf(),
        });
        let first = probe_server_identity(Arc::clone(&runner), Duration::from_secs(10))
            .await
            .expect("first probe");
        let second = probe_server_identity(Arc::clone(&runner), Duration::from_secs(10))
            .await
            .expect("second probe");
        assert_eq!(first.source_server_id, second.source_server_id);
        assert!(!first.source_server_id.is_empty());
        let marker = std::fs::read_to_string(temp.path().join(".escode/server/install.json"))
            .expect("marker");
        let value: Value = serde_json::from_str(&marker).expect("json");
        assert_eq!(value["product"], "escode-server");
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["installationId"], first.source_server_id);
        assert_eq!(
            value["canonicalServerRoot"],
            temp.path()
                .join(".escode/server")
                .to_string_lossy()
                .as_ref()
        );
    }

    #[tokio::test]
    async fn reads_existing_marker_without_rewriting() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(".escode/server");
        std::fs::create_dir_all(&root).expect("root");
        let marker = root.join("install.json");
        let existing = r#"{"installationId":"22222222-2222-4222-8222-222222222222"}"#;
        std::fs::write(&marker, existing).expect("marker");
        let runner: Arc<dyn RemoteRunner> = Arc::new(ShRunner {
            home: temp.path().to_path_buf(),
        });
        let identity = probe_server_identity(runner, Duration::from_secs(10))
            .await
            .expect("probe");
        assert_eq!(
            identity.source_server_id,
            "22222222-2222-4222-8222-222222222222"
        );
        assert_eq!(std::fs::read_to_string(marker).expect("marker"), existing);
    }
}
