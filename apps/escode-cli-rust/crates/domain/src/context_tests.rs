//! 压缩边界持久化（docs/specs/rust-tool-search.md §5.2）。

use crate::context::ContextState;

#[test]
fn legacy_context_without_boundary_reads_as_empty_names() {
    let state: ContextState = serde_json::from_str(r#"{"offset":3,"summary":"s"}"#).unwrap();
    assert_eq!(state.offset, 3);
    assert!(state.pre_compact_discovered_tools.is_empty());
}

#[test]
fn boundary_names_roundtrip_and_empty_names_are_omitted() {
    let state = ContextState {
        offset: 1,
        summary: None,
        pre_compact_discovered_tools: vec!["b".into(), "a".into()],
    };
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(
        json["preCompactDiscoveredTools"],
        serde_json::json!(["b", "a"])
    );
    assert_eq!(
        serde_json::from_value::<ContextState>(json)
            .unwrap()
            .pre_compact_discovered_tools,
        vec!["b".to_owned(), "a".to_owned()]
    );
    let empty = ContextState {
        offset: 1,
        summary: None,
        pre_compact_discovered_tools: vec![],
    };
    assert!(
        serde_json::to_value(&empty)
            .unwrap()
            .get("preCompactDiscoveredTools")
            .is_none()
    );
}
