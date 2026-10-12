//! LLM Wiki 知识条目 schema 与一致性校验（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §5）。
//! 纯数据：时间戳由调用方传入（domain 不读时钟）。
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const SCHEMA_VERSION: u32 = 1;
pub const ID_MAX_LEN: usize = 128;
pub const EXCERPT_MAX_CHARS: usize = 600;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $text)] $variant),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(self) -> &'static str { match self { $($name::$variant => $text),+ } }
            pub fn parse(text: &str) -> Option<Self> { match text { $($text => Some($name::$variant),)+ _ => None } }
        }
    };
}

string_enum!(Scope { Global => "global", Project => "project" });
string_enum!(Kind {
    Standard => "standard", Architecture => "architecture", Adr => "adr", Contract => "contract",
    BusinessRule => "business-rule", Invariant => "invariant", Guide => "guide",
    Reference => "reference", Pattern => "pattern", AntiPattern => "anti-pattern", Proposal => "proposal",
});
string_enum!(Status {
    Proposal => "proposal", Approved => "approved", NeedsReview => "needs-review",
    Deprecated => "deprecated", Superseded => "superseded", Rejected => "rejected",
});
// system-safety 与 user-instruction 不属于 Wiki：枚举中不存在，反序列化即失败（规则 5）。
string_enum!(Authority {
    ApprovedGlobalStandard => "approved-global-standard",
    ApprovedProjectStandard => "approved-project-standard",
    ApprovedAdrContract => "approved-adr-contract",
    VerifiedReference => "verified-reference",
    ObservedEvidence => "observed-evidence",
    HistoricalReview => "historical-review",
    Proposal => "proposal",
});
string_enum!(Validity { Valid => "valid", Stale => "stale", BrokenSource => "broken-source", Unverified => "unverified" });
string_enum!(Origin { Imported => "imported", UserAuthored => "user-authored", AgentProposal => "agent-proposal", ReviewFinding => "review-finding" });

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub document_id: String,
    pub source_path: String,
    pub content_hash: String,
    pub format: String,
    pub locator: Locator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    pub parser: String,
    pub parser_version: String,
    pub extracted_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidityInfo {
    pub state: Validity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppliesTo {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewEvent {
    pub action: String,
    pub at: String,
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// frontmatter 的结构化部分；未知字段收进 `extra` 原样回写（向后兼容）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub kind: Kind,
    pub status: Status,
    pub authority: Authority,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "is_default_applies")]
    pub applies_to: AppliesTo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceRef>,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    pub validity: ValidityInfo,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review: Vec<ReviewEvent>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn is_default_applies(value: &AppliesTo) -> bool {
    value.paths.is_empty() && value.versions.is_none()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub meta: Meta,
    pub body: String,
}

impl Entry {
    /// 条目版本：去重与缓存都按 `id@updated_at` 判断「是否同一份内容」。
    pub fn version(&self) -> &str {
        &self.meta.updated_at
    }
    /// 可自动注入：已批准或待复核（后者在 reminder 里标注）。
    pub fn injectable(&self) -> bool {
        matches!(self.meta.status, Status::Approved | Status::NeedsReview)
    }
    /// 已退出检索面的终态。
    pub fn retired(&self) -> bool {
        matches!(
            self.meta.status,
            Status::Deprecated | Status::Superseded | Status::Rejected
        )
    }
}

pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (2..=ID_MAX_LEN).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && !bytes[0].is_ascii_uppercase()
        && bytes.iter().all(|b| {
            b.is_ascii_digit() || b.is_ascii_lowercase() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// 一致性校验；`expected_project` 为条目所在存储的项目（global 存储传 None）。
pub fn validate(meta: &Meta, expected_project: Option<&str>) -> Vec<String> {
    let mut issues = vec![];
    if meta.schema_version != SCHEMA_VERSION {
        issues.push(format!(
            "unsupported schema_version {}",
            meta.schema_version
        ));
    }
    if !valid_id(&meta.id) {
        issues.push(format!("invalid id '{}'", meta.id));
    }
    if meta.title.trim().is_empty() {
        issues.push("title is empty".into());
    }
    match (meta.scope, meta.project_id.as_deref(), expected_project) {
        (Scope::Global, Some(_), _) => issues.push("global entry must not carry project_id".into()),
        (Scope::Global, None, Some(_)) => {
            issues.push("global entry stored in a project wiki".into())
        }
        (Scope::Project, None, _) => issues.push("project entry requires project_id".into()),
        (Scope::Project, Some(found), Some(expected)) if found != expected => issues.push(format!(
            "project_id '{found}' does not match store project '{expected}'"
        )),
        (Scope::Project, Some(_), None) => {
            issues.push("project entry stored in the global wiki".into())
        }
        _ => {}
    }
    let approved_authority = matches!(
        meta.authority,
        Authority::ApprovedGlobalStandard
            | Authority::ApprovedProjectStandard
            | Authority::ApprovedAdrContract
            | Authority::VerifiedReference
    );
    if meta.status == Status::Approved && (!approved_authority || meta.kind == Kind::Proposal) {
        issues.push("approved entry requires an approved authority and a non-proposal kind".into());
    }
    if meta.status == Status::Proposal
        && !matches!(
            meta.authority,
            Authority::Proposal | Authority::ObservedEvidence | Authority::HistoricalReview
        )
    {
        issues.push("proposal entry cannot claim an approved authority".into());
    }
    if let Some(confidence) = meta.confidence
        && !(0.0..=1.0).contains(&confidence)
    {
        issues.push("confidence must be within 0..1".into());
    }
    for source in &meta.sources {
        if source
            .excerpt
            .as_ref()
            .is_some_and(|e| e.chars().count() > EXCERPT_MAX_CHARS)
        {
            issues.push(format!(
                "source excerpt for {} exceeds {EXCERPT_MAX_CHARS} chars",
                source.document_id
            ));
        }
    }
    issues
}

/// 批准后的权威等级：按作用域与类型推导（adr / contract 归入决策契约档）。
pub fn approved_authority(scope: Scope, kind: Kind) -> Authority {
    match (scope, kind) {
        (_, Kind::Adr | Kind::Contract) => Authority::ApprovedAdrContract,
        (_, Kind::Reference) => Authority::VerifiedReference,
        (Scope::Global, _) => Authority::ApprovedGlobalStandard,
        (Scope::Project, _) => Authority::ApprovedProjectStandard,
    }
}

/// 权威等级在排序中的权重（等级不证明内容正确，只影响同等相关度下的先后）。
pub fn authority_weight(authority: Authority) -> f64 {
    match authority {
        Authority::ApprovedGlobalStandard | Authority::ApprovedProjectStandard => 1.0,
        Authority::ApprovedAdrContract => 1.0,
        Authority::VerifiedReference => 0.95,
        Authority::ObservedEvidence => 0.8,
        Authority::HistoricalReview => 0.7,
        Authority::Proposal => 0.6,
    }
}
