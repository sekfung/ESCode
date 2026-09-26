use super::*;
use serde_json::json;

#[test]
fn success_trims_leading_blank_lines_and_trailing_space() {
    assert_eq!(format(&json!({"stdout": "\n  \nhello\n  world \n\n", "stderr": "", "status": "completed", "exitCode": 0})), "hello\n  world");
    assert_eq!(format(&json!({"stdout": "", "stderr": "", "status": "completed", "exitCode": 0})), "");
}

#[test]
fn provider_error_prefixes_exit_code_but_semantic_no_match_does_not() {
    let failed = json!({"stdout": "out", "stderr": " boom ", "status": "failed", "exitCode": 2,
        "returnCodeInterpretation": "Command exited with code 2"});
    assert_eq!(format(&failed), "Exit code 2\nout\nboom");
    let no_match = json!({"stdout": "", "stderr": "", "status": "failed", "exitCode": 1,
        "returnCodeInterpretation": return_code_interpretation("rg needle src", "failed", Some(1))});
    assert_eq!(no_match["returnCodeInterpretation"], "No matches found");
    assert_eq!(format(&no_match), "");
}

#[test]
fn interrupted_commands_are_marked() {
    let timed_out = json!({"stdout": "partial\n", "stderr": "", "status": "timed_out", "interrupted": true});
    assert_eq!(format(&timed_out), "partial\n<error>Command was aborted before completion</error>");
}

#[test]
fn background_messages_follow_ts_wording() {
    let started = json!({"stdout": "", "stderr": "", "status": "backgrounded", "backgroundTaskId": "t1",
        "persistedOutputPath": "/o/t1.output", "backgroundedByUser": false});
    assert_eq!(
        format(&started),
        "Command running in background with ID: t1. Output is being written to: /o/t1.output. You will be notified when it completes. To check interim output, use Read on that file path."
    );
    let by_user = json!({"stdout": "", "stderr": "", "status": "backgrounded", "backgroundTaskId": "t1",
        "persistedOutputPath": "/o/t1.output", "backgroundedByUser": true});
    assert_eq!(format(&by_user), "Command was manually backgrounded by user with ID: t1. Output is being written to: /o/t1.output");
}

#[test]
fn persisted_output_uses_envelope_with_preview() {
    let stdout = format!("{}\n{}", "a".repeat(1500), "b".repeat(1000));
    let data = json!({"stdout": stdout, "stderr": "", "status": "completed", "exitCode": 0,
        "persistedOutputPath": "/o/x.output", "persistedOutputSize": 40_000});
    assert_eq!(
        format(&data),
        format!("<persisted-output>\nOutput too large (39.1KB). Full output saved to: /o/x.output\n\nPreview (first 2KB):\n{}\n...\n</persisted-output>", "a".repeat(1500))
    );
    assert_eq!(byte_size(512), "512 bytes");
    assert_eq!(byte_size(2 * 1024 * 1024), "2MB");
}

#[test]
fn return_codes_and_silent_commands() {
    assert_eq!(return_code_interpretation("rg x && test -f y", "failed", Some(1)).as_deref(), Some("Condition is false"));
    assert_eq!(return_code_interpretation("git -C repo diff --quiet", "failed", Some(1)).as_deref(), Some("Files differ"));
    assert_eq!(return_code_interpretation("make", "failed", Some(1)).as_deref(), Some("Command exited with code 1"));
    assert_eq!(return_code_interpretation("grep x", "failed", Some(2)).as_deref(), Some("Command exited with code 2"));
    assert_eq!(return_code_interpretation("sleep 9", "timed_out", None).as_deref(), Some("Command timed out"));
    assert_eq!(return_code_interpretation("ls", "completed", Some(0)), None);
    assert!(is_silent("mkdir -p a && cd a"));
    assert!(is_silent("rm -f x || true"));
    assert!(!is_silent("ls"));
    assert!(!is_silent("mkdir $DIR"));
}

#[test]
fn stop_messages_match_ts() {
    assert_eq!(stop_message("timed_out", Some(1_000), false).as_deref(), Some("Command timed out after 1s"));
    assert_eq!(timeout_duration(1_500), "1.5s");
    assert_eq!(timeout_duration(120_000), "2m");
    assert_eq!(timeout_duration(250), "250ms");
    assert_eq!(stop_message("cancelled", None, false).as_deref(), Some("Execution cancelled"));
    assert_eq!(stop_message("failed", None, false), None);
}
