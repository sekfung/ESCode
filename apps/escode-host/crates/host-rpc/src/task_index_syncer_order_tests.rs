//! syncer 帧应用的写序回归（评审 R2）：连续摘要的索引写必须按帧到达序落库。
use serde_json::json;

use super::tests::{
    index_wire, ingest_with_index, next_event, snapshot_payload, summary, task_meta_sync, wait_for,
};
use crate::agent_v4::frames::TopicKind;
use crate::task_index_syncer::topic as sync_topic;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consecutive_title_changes_land_in_frame_order() {
    // 评审 R2：帧应用对每条摘要各自 spawn 写时，连续改名可能乱序落库（索引停在旧标题）。
    let (_dir, ingest, mut events) = ingest_with_index();
    sync_topic::set_subscription_for_tests(&ingest, TopicKind::SessionsIndex, "sub-1", "e1");
    sync_topic::handle_wire(
        &ingest,
        TopicKind::SessionsIndex,
        &index_wire(
            "sub-1",
            0,
            1,
            "initial",
            snapshot_payload(vec![summary("s1", "running", "t-start")]),
        ),
    );
    wait_for(|| task_meta_sync(&ingest, "s1"), "baseline seed row").await;
    for round in 0..50_u64 {
        sync_topic::handle_wire(
            &ingest,
            TopicKind::SessionsIndex,
            &index_wire(
                "sub-1",
                round + 1,
                round + 2,
                "online",
                json!({"kind": "deltas", "deltas": [
                    {"op": "session.upserted", "session": summary("s1", "running", &format!("t-{round:02}"))},
                ]}),
            ),
        );
    }
    for round in 0..50_u64 {
        let event = next_event(&mut events, "task_title_changed").await;
        assert_eq!(
            event["taskMeta"]["title"],
            format!("t-{round:02}"),
            "events in frame order"
        );
    }
    let last = task_meta_sync(&ingest, "s1").expect("row");
    assert_eq!(last["title"], "t-49", "index ends on the newest title");
}
