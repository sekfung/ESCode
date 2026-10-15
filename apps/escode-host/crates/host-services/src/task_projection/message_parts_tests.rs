use super::map_prompt_attachment_from_file_part;
use crate::task_projection::message_timeline::{
    extract_synthetic_timeline_from_text_part, synthesize_compaction_timeline,
};
use serde_json::json;

#[test]
fn file_parts_project_image_audio_and_file_attachments() {
    let image = json!({"type": "file", "url": "data:image/png;base64,QUJD", "mime": "image/png",
        "filename": "  chart.png ", "metadata": {"sizeBytes": 12}});
    assert_eq!(
        map_prompt_attachment_from_file_part(&image),
        Some(
            json!({"kind": "image", "filename": "chart.png", "mimeType": "image/png",
            "sizeBytes": 12, "dataBase64": "QUJD"})
        )
    );
    // audio 不投影 sizeBytes；localPath 取 metadata.originalUrl。
    let audio = json!({"type": "file", "url": "file:///tmp/voice.ogg", "mime": "audio/ogg",
        "metadata": {"originalUrl": "/tmp/voice.ogg", "sizeBytes": 9}});
    assert_eq!(
        map_prompt_attachment_from_file_part(&audio),
        Some(
            json!({"kind": "audio", "filename": "voice.ogg", "mimeType": "audio/ogg",
            "localPath": "/tmp/voice.ogg"})
        )
    );
    // file 的 sizeBytes 恒存在（缺值补 0），无本地路径时回填 preview.text。
    let file = json!({"type": "file", "url": "https://example.com/notes.txt?x=1", "mime": "text/plain",
        "metadata": {"preview": {"text": "hello"}}});
    assert_eq!(
        map_prompt_attachment_from_file_part(&file),
        Some(
            json!({"kind": "file", "filename": "notes.txt", "mimeType": "text/plain",
            "sizeBytes": 0, "textContent": "hello"})
        )
    );
}

#[test]
fn text_part_fork_context_projects_session_fork_timeline() {
    let part = json!({"type": "text", "text": "forked", "metadata": {"forkContext": {
        "kind": "session_fork", "parentSessionId": "p1", "targetMessageId": "m2",
        "targetCheckpointId": "", "restoredFileCount": 3}}});
    assert_eq!(
        extract_synthetic_timeline_from_text_part(&part),
        Some(
            json!({"version": 1, "kind": "synthetic", "type": "session_fork",
            "display": "separator", "parentSessionId": "p1", "targetMessageId": "m2",
            "restoredFileCount": 3})
        )
    );
    let incomplete = json!({"type": "text", "text": "x",
        "metadata": {"forkContext": {"kind": "session_fork", "targetMessageId": "m2"}}});
    assert_eq!(extract_synthetic_timeline_from_text_part(&incomplete), None);
}

#[test]
fn compaction_part_synthesizes_context_compaction_timeline() {
    let part = json!({"type": "compaction", "partId": "part-9", "auto": true,
        "summaryMessageId": "summary-1",
        "metadata": {"timelineStatus": "started", "trigger": "reactive", "replace": false,
            "preCompactTokenCount": 10, "attempt": 2}});
    assert_eq!(
        synthesize_compaction_timeline(&part),
        Some(
            json!({"version": 1, "kind": "synthetic", "type": "context_compaction",
            "operationId": "part-9", "status": "started", "trigger": "reactive",
            "display": "separator", "replace": false, "boundaryId": "summary-1",
            "summaryMessageId": "summary-1", "preCompactTokenCount": 10, "attempt": 2})
        )
    );
    // compact summary 带 compaction metadata 但没有 timeline 身份 → 不合成横线。
    let context_only = json!({"type": "compaction", "partId": "part-9", "auto": false,
        "metadata": {"operationId": "op-1"}});
    assert_eq!(synthesize_compaction_timeline(&context_only), None);
    // 只有 summaryMessageId → status 缺省 completed、trigger 回落 auto。
    let auto = json!({"type": "compaction", "partId": "p", "auto": true,
        "summaryMessageId": "s", "metadata": {}});
    let timeline = match synthesize_compaction_timeline(&auto) {
        Some(timeline) => timeline,
        None => panic!("expected compaction timeline"),
    };
    assert_eq!(timeline.get("status"), Some(&json!("completed")));
    assert_eq!(timeline.get("trigger"), Some(&json!("auto")));
}
