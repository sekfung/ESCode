use std::sync::Arc;
use std::time::Duration;

use escode_host_core::rpc_serialization::WireValue;
use escode_host_core::rpc_wire::FrameDecoder;
use serde_json::json;
use tokio::sync::mpsc;

use super::*;
use crate::source_server::{SourceServerEvent, SourceServerRegistry};
use crate::source_server_client::{
    RESPONSE_INITIALIZE, RESPONSE_PROMISE_ERROR, RESPONSE_PROMISE_SUCCESS, RemoteChannelClient,
};

const SOURCE: &str = "33333333-3333-4333-8333-333333333333";
const PROFILE: &str = "44444444-4444-4444-8444-444444444444";

fn registry() -> Arc<SourceServerRegistry> {
    Arc::new(
        SourceServerRegistry::from_settings(&json!({
            "sourceServers": [{ "sourceServerId": SOURCE, "displayName": "fixture" }],
            "connectionProfiles": [{
                "profileId": PROFILE,
                "sourceServerId": SOURCE,
                "transport": "ssh",
                "target": {"kind": "ssh", "host": "fixture", "username": "dev"},
                "createdAt": 1_u64,
            }],
            "sourceServerWorkspaceHistory": [],
        }))
        .expect("registry"),
    )
}

fn token(attempt: u64) -> MigrationConnectionToken {
    MigrationConnectionToken {
        profile_id: PROFILE.to_owned(),
        attempt,
    }
}

fn initialized_client(
    channels: &[&str],
) -> (Arc<RemoteChannelClient>, mpsc::UnboundedReceiver<Vec<u8>>) {
    let (outbound, receiver) = mpsc::unbounded_channel();
    let client = Arc::new(RemoteChannelClient::new(outbound));
    let mut payload = WireValue::Array(vec![WireValue::Int(RESPONSE_INITIALIZE)]).encode();
    WireValue::Object(json!({ "channels": channels })).encode_into(&mut payload);
    client.handle_frame(&payload).expect("initialize");
    (client, receiver)
}

async fn next_request(receiver: &mut mpsc::UnboundedReceiver<Vec<u8>>) -> (i32, String) {
    let frame = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("request within timeout")
        .expect("outbound open");
    let mut decoder = FrameDecoder::new();
    decoder.push(&frame);
    let payload = decoder
        .next_frame()
        .expect("frame decodes")
        .expect("complete frame");
    let (header, _) = WireValue::decode(&payload).expect("header decodes");
    let WireValue::Array(items) = header else {
        panic!("request header must be an array");
    };
    let WireValue::Int(id) = items[1] else {
        panic!("request id");
    };
    let WireValue::String(method) = items[3].clone() else {
        panic!("request method");
    };
    (id, method)
}

fn respond(client: &RemoteChannelClient, kind: i32, id: i32, body: WireValue) {
    let mut payload = WireValue::Array(vec![WireValue::Int(kind), WireValue::Int(id)]).encode();
    body.encode_into(&mut payload);
    client.handle_frame(&payload).expect("response frame");
}

fn lookup(client: Arc<RemoteChannelClient>, attempt: u64) -> MigrationConnectionLookup {
    Arc::new(move |_| {
        Some(ActiveMigrationConnection {
            token: token(attempt),
            client: Arc::clone(&client),
        })
    })
}

async fn next_directory_generation(
    events: &mut tokio::sync::broadcast::Receiver<SourceServerEvent>,
) -> u64 {
    loop {
        match tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("event within timeout")
            .expect("event")
        {
            SourceServerEvent::Directory(generation) => return generation,
            _ => continue,
        }
    }
}

async fn answer_counts(
    client: &RemoteChannelClient,
    receiver: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    failed: i32,
    abandoned: i32,
) {
    for _ in 0..2 {
        let (id, method) = next_request(receiver).await;
        let value = if method == "getFailedPathCount" {
            failed
        } else {
            abandoned
        };
        respond(client, RESPONSE_PROMISE_SUCCESS, id, WireValue::Int(value));
    }
}

#[test]
fn offline_source_is_requires_connection_without_fake_zero() {
    let registry = registry();
    let counts = Arc::new(SourceServerMigrationCounts::new(registry));
    let lookup: MigrationConnectionLookup = Arc::new(|_| None);
    let projection = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(
        projection.migration_counts,
        vec![json!({ "sourceServerId": SOURCE, "status": "requires-connection" })]
    );
    assert_eq!(projection.failed_path_count, 0);
    assert_eq!(projection.abandoned_path_count, 0);
}

#[tokio::test]
async fn loading_then_available_publishes_directory_generation() {
    let registry = registry();
    let counts = Arc::new(SourceServerMigrationCounts::new(Arc::clone(&registry)));
    let (client, mut receiver) = initialized_client(&[SOURCE_SERVER_WORKSPACE_CHANNEL]);
    let lookup = lookup(Arc::clone(&client), 1);
    let mut events = registry.subscribe();

    let first = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(first.migration_counts[0]["status"], json!("loading"));
    assert_eq!(first.failed_path_count, 0);

    answer_counts(&client, &mut receiver, 2, 3).await;
    assert_eq!(next_directory_generation(&mut events).await, 2);

    let second = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(second.migration_counts[0]["status"], json!("available"));
    assert_eq!(second.migration_counts[0]["failedPathCount"], json!(2));
    assert_eq!(second.migration_counts[0]["abandonedPathCount"], json!(3));
    assert_eq!(second.failed_path_count, 2);
    assert_eq!(second.abandoned_path_count, 3);
}

#[tokio::test]
async fn one_failed_count_marks_unavailable() {
    let registry = registry();
    let counts = Arc::new(SourceServerMigrationCounts::new(Arc::clone(&registry)));
    let (client, mut receiver) = initialized_client(&[SOURCE_SERVER_WORKSPACE_CHANNEL]);
    let lookup = lookup(Arc::clone(&client), 1);
    let mut events = registry.subscribe();
    let _ = counts.read(&[SOURCE.to_owned()], &lookup);

    let mut failed_id = None;
    let mut abandoned_id = None;
    for _ in 0..2 {
        let (id, method) = next_request(&mut receiver).await;
        if method == "getFailedPathCount" {
            failed_id = Some(id);
        } else {
            abandoned_id = Some(id);
        }
    }
    respond(
        &client,
        RESPONSE_PROMISE_SUCCESS,
        failed_id.expect("failed request"),
        WireValue::Int(7),
    );
    respond(
        &client,
        RESPONSE_PROMISE_ERROR,
        abandoned_id.expect("abandoned request"),
        WireValue::Object(json!({ "name": "Error", "message": "boom" })),
    );
    let _ = next_directory_generation(&mut events).await;

    let projection = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(
        projection.migration_counts[0]["status"],
        json!("unavailable")
    );
    assert_eq!(projection.failed_path_count, 0);
}

#[tokio::test]
async fn missing_channel_marks_unavailable_without_rpc() {
    let registry = registry();
    let counts = Arc::new(SourceServerMigrationCounts::new(Arc::clone(&registry)));
    let (client, mut receiver) = initialized_client(&["test"]);
    let lookup = lookup(Arc::clone(&client), 1);
    let mut events = registry.subscribe();
    let first = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(first.migration_counts[0]["status"], json!("loading"));
    assert_eq!(next_directory_generation(&mut events).await, 2);
    assert!(
        receiver.try_recv().is_err(),
        "must not call an unadvertised channel"
    );
    let projection = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(
        projection.migration_counts[0]["status"],
        json!("unavailable")
    );
    assert_eq!(projection.failed_path_count, 0);
}

#[tokio::test]
async fn in_flight_invalidation_merges_into_one_reread() {
    let registry = registry();
    let counts = Arc::new(SourceServerMigrationCounts::new(Arc::clone(&registry)));
    let (client, mut receiver) = initialized_client(&[SOURCE_SERVER_WORKSPACE_CHANNEL]);
    let lookup = lookup(Arc::clone(&client), 1);
    let mut events = registry.subscribe();
    let _ = counts.read(&[SOURCE.to_owned()], &lookup);

    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(next_request(&mut receiver).await);
    }
    counts.invalidate(Some(SOURCE));
    for (id, _) in ids {
        respond(&client, RESPONSE_PROMISE_SUCCESS, id, WireValue::Int(1));
    }
    answer_counts(&client, &mut receiver, 5, 6).await;
    let _ = next_directory_generation(&mut events).await;

    let projection = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(projection.migration_counts[0]["status"], json!("available"));
    assert_eq!(projection.failed_path_count, 5);
    assert_eq!(projection.abandoned_path_count, 6);
}

#[tokio::test]
async fn late_result_after_connection_change_is_not_published() {
    let registry = registry();
    let counts = Arc::new(SourceServerMigrationCounts::new(Arc::clone(&registry)));
    let (client_a, mut receiver) = initialized_client(&[SOURCE_SERVER_WORKSPACE_CHANNEL]);
    let (client_b, _receiver_b) = initialized_client(&[SOURCE_SERVER_WORKSPACE_CHANNEL]);
    let current = Arc::new(std::sync::Mutex::new(Some(ActiveMigrationConnection {
        token: token(1),
        client: Arc::clone(&client_a),
    })));
    let current_for_lookup = Arc::clone(&current);
    let lookup: MigrationConnectionLookup = Arc::new(move |_| {
        current_for_lookup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    });
    let mut events = registry.subscribe();
    let _ = counts.read(&[SOURCE.to_owned()], &lookup);

    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(next_request(&mut receiver).await);
    }
    *current
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ActiveMigrationConnection {
        token: token(2),
        client: client_b,
    });
    for (id, _) in ids {
        respond(&client_a, RESPONSE_PROMISE_SUCCESS, id, WireValue::Int(9));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        events.try_recv().is_err(),
        "stale connection result must not publish a directory change"
    );
    let projection = counts.read(&[SOURCE.to_owned()], &lookup);
    assert_eq!(projection.migration_counts[0]["status"], json!("loading"));
    assert_eq!(projection.failed_path_count, 0);
}

#[test]
fn snapshot_projection_totals_only_available_entries() {
    let projection = projection_from_entries(vec![
        json!({"sourceServerId": "a", "status": "available", "failedPathCount": 2, "abandonedPathCount": 0}),
        json!({"sourceServerId": "b", "status": "unavailable"}),
        json!({"sourceServerId": "c", "status": "available", "failedPathCount": 0, "abandonedPathCount": 4}),
    ]);
    assert_eq!(projection.failed_path_count, 2);
    assert_eq!(projection.abandoned_path_count, 4);
}
