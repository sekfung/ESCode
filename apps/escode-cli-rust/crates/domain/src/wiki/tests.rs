use super::entry::*;
use super::*;
use serde_json::Map;

pub(super) fn entry(id: &str, scope: Scope, status: Status, title: &str, body: &str) -> Entry {
    let authority = match status {
        Status::Approved => approved_authority(scope, Kind::Standard),
        _ => Authority::Proposal,
    };
    Entry {
        meta: Meta {
            schema_version: SCHEMA_VERSION,
            id: id.into(),
            title: title.into(),
            scope,
            project_id: (scope == Scope::Project).then(|| "proj-a".into()),
            kind: Kind::Standard,
            status,
            authority,
            tags: vec![],
            related: vec![],
            supersedes: None,
            superseded_by: None,
            applies_to: AppliesTo::default(),
            sources: vec![],
            origin: Origin::UserAuthored,
            confidence: None,
            validity: ValidityInfo {
                state: Validity::Valid,
                reason: None,
            },
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            verified_at: None,
            review: vec![],
            extra: Map::new(),
        },
        body: body.into(),
    }
}

#[test]
fn frontmatter_round_trip_is_byte_stable_and_keeps_unknown_fields() {
    let mut e = entry(
        "rule-api-errors",
        Scope::Project,
        Status::Approved,
        "API: Error Handling",
        "Handlers must use the standard error format.\n",
    );
    e.meta.tags = vec!["api".into(), "true".into()];
    e.meta
        .extra
        .insert("x_custom".into(), serde_json::json!({"keep": 1}));
    e.meta.sources.push(SourceRef {
        document_id: "doc-1".into(),
        source_path: "docs/api.md".into(),
        content_hash: "sha256:ab".into(),
        format: "markdown".into(),
        locator: Locator {
            kind: "heading".into(),
            value: "API / Errors".into(),
        },
        excerpt: Some("multi\nline: excerpt".into()),
        parser: "markdown".into(),
        parser_version: "1".into(),
        extracted_at: "2026-01-01T00:00:00Z".into(),
    });
    let text = frontmatter::serialize(&e);
    let parsed = frontmatter::parse(&text).expect("parse");
    assert_eq!(parsed, e);
    assert_eq!(frontmatter::serialize(&parsed), text);
}

#[test]
fn schema_rejects_reserved_authorities_and_scope_mismatch() {
    let text = "---\nschema_version: 1\nid: x1\ntitle: t\nscope: global\nkind: standard\nstatus: approved\nauthority: system-safety\norigin: user-authored\nvalidity:\n  state: valid\ncreated_at: a\nupdated_at: a\n---\nbody\n";
    assert!(matches!(
        frontmatter::parse(text),
        Err(frontmatter::ParseError::InvalidMeta(_))
    ));
    let e = entry("ab", Scope::Project, Status::Approved, "t", "b");
    assert!(validate(&e.meta, Some("proj-a")).is_empty());
    assert!(
        !validate(&e.meta, Some("proj-b")).is_empty(),
        "project mismatch must fail"
    );
    assert!(
        !validate(&e.meta, None).is_empty(),
        "project entry in global store must fail"
    );
    let mut p = entry("ab", Scope::Global, Status::Proposal, "t", "b");
    p.meta.authority = Authority::ApprovedGlobalStandard;
    assert!(
        !validate(&p.meta, None).is_empty(),
        "proposal cannot claim approved authority"
    );
    assert!(!valid_id("Bad Id") && valid_id("rule-1.a_b"));
}

#[test]
fn tokenizer_splits_identifiers_and_cjk() {
    let t = text::tokenize("parseHTTPResponse snake_case 错误处理");
    for want in [
        "parsehttpresponse",
        "parse",
        "http",
        "response",
        "snake",
        "case",
        "错误",
        "误处",
        "处理",
    ] {
        assert!(t.contains(&want.to_string()), "missing {want}: {t:?}");
    }
    assert!(!text::tokenize("the and of").iter().any(|x| x == "the"));
    assert_eq!(
        text::tokenize("errors returned policies status"),
        vec!["error", "return", "policy", "status"]
    );
}

#[test]
fn workflow_classification() {
    use workflow::{Workflow, classify};
    assert_eq!(classify("Please review this PR diff"), Workflow::Review);
    assert_eq!(classify("帮我评审一下这次改动"), Workflow::Review);
    assert_eq!(classify("fix the crash in parser"), Workflow::Debugging);
    assert_eq!(classify("add unit tests for the cache"), Workflow::Testing);
    assert_eq!(classify("refactor the module"), Workflow::Refactoring);
    assert_eq!(classify("implement a new endpoint"), Workflow::Coding);
    assert_eq!(classify("hello there"), Workflow::General);
    assert_eq!(
        classify("check the address field"),
        Workflow::General,
        "word boundary: address ≠ add"
    );
    assert_eq!(
        classify("implement the error handling for the orders handler"),
        Workflow::Coding,
        "topical 'error' is weak"
    );
    assert_eq!(
        classify("the build fails with an error"),
        Workflow::Debugging
    );
}

fn index_of(entries: &[Entry]) -> index::Index {
    index::Index::build(entries.iter())
}

#[test]
fn search_ranks_relevant_entries_and_excludes_proposals_by_default() {
    let entries = vec![
        entry(
            "err",
            Scope::Project,
            Status::Approved,
            "API error handling",
            "Handlers must return the standard error envelope.",
        ),
        entry(
            "log",
            Scope::Project,
            Status::Approved,
            "Logging",
            "Use the service logger.",
        ),
        entry(
            "prop",
            Scope::Project,
            Status::Proposal,
            "Error budget proposal",
            "error error error",
        ),
    ];
    let idx = index_of(&entries);
    let q = rank::Query::new(
        "how should API errors be returned",
        Some(workflow::Workflow::Coding),
        vec![],
    );
    let hits = rank::search(&idx, &q);
    assert_eq!(hits.first().map(|h| h.id.as_str()), Some("err"));
    assert!(hits.iter().all(|h| h.id != "prop"));
    let mut with = q;
    with.include_proposals = true;
    assert!(rank::search(&idx, &with).iter().any(|h| h.id == "prop"));
    assert!(
        rank::search(
            &idx,
            &rank::Query::new("kubernetes helm chart", None, vec![])
        )
        .is_empty()
    );
}

#[test]
fn gate_blocks_weak_matches() {
    let entries = vec![entry(
        "x",
        Scope::Global,
        Status::Approved,
        "Database migrations",
        "Run migrations with the migrate tool before deploy. Large body text about many other things.",
    )];
    let idx = index_of(&entries);
    let strong = rank::search(&idx, &rank::Query::new("database migrations", None, vec![]));
    assert!(rank::passes_gate(&strong[0], 0.3));
    let weak = rank::search(&idx, &rank::Query::new("things", None, vec![]));
    assert!(weak.iter().all(|h| !rank::passes_gate(h, 0.3)), "{weak:?}");
}

#[test]
fn index_incremental_update_and_fingerprint() {
    let mut entries = vec![entry(
        "a1",
        Scope::Global,
        Status::Approved,
        "Alpha rule",
        "alpha body",
    )];
    let mut idx = index_of(&entries);
    let before = (idx.version, idx.fingerprint.clone());
    entries[0].meta.updated_at = "2026-02-01T00:00:00Z".into();
    entries[0].body = "beta body".into();
    idx.upsert(&entries[0]);
    assert!(idx.version > before.0 && idx.fingerprint != before.1);
    assert!(
        idx.postings
            .get("alpha")
            .is_some_and(|p| p.iter().any(|(id, _)| id == "a1")),
        "title term kept"
    );
    assert!(idx.postings.contains_key("beta"));
    assert!(!idx.postings.get("body").unwrap().is_empty());
    idx.remove("a1");
    assert!(idx.docs.is_empty() && idx.postings.is_empty());
    let rebuilt = index_of(&[]);
    assert_eq!(rebuilt.fingerprint, idx.fingerprint);
}

#[test]
fn applies_to_paths_boost_and_glob() {
    assert!(rank::glob_match("src/api/**", "src/api/v1/handler.rs"));
    assert!(rank::glob_match("*.rs", "crates/core/src/lib.rs"));
    assert!(!rank::glob_match("src/*.rs", "src/a/b.rs"));
    let mut a = entry(
        "a",
        Scope::Project,
        Status::Approved,
        "Handler rules",
        "Handlers must validate input.",
    );
    a.meta.applies_to.paths = vec!["src/api/**".into()];
    let b = entry(
        "b",
        Scope::Project,
        Status::Approved,
        "Handler rules",
        "Handlers must validate input.",
    );
    let idx = index_of(&[a, b]);
    let hits = rank::search(
        &idx,
        &rank::Query::new("handler", None, vec!["src/api/users.rs".into()]),
    );
    assert_eq!(hits[0].id, "a");
}

#[test]
fn excerpt_picks_relevant_paragraphs() {
    let body = format!(
        "{}\n\nThe cache must be invalidated on write.\n\n{}",
        "intro ".repeat(400),
        "outro ".repeat(400)
    );
    let (excerpt, truncated) =
        rank::best_excerpt(&body, &["cache".into(), "invalidated".into()], 50);
    assert!(truncated && excerpt.contains("cache must be invalidated"));
}
