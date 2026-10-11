use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionItem {
    pub label: String,
    pub description: String,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub preview: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<OptionItem>,
    #[serde(default)]
    pub multi_select: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub preview: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub notes: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub source: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionInput {
    pub questions: Vec<Question>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub answers: Option<BTreeMap<String, String>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub annotations: Option<BTreeMap<String, Annotation>>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub metadata: Option<Metadata>,
}
impl QuestionInput {
    pub fn parse(value: Value) -> Result<Self> {
        let input = Self::structural(value)?;
        input.refine()?;
        Ok(input)
    }
    /// 结构与数量约束（TS 中由 JSON Schema 拦截，以工具错误收口）。
    pub fn structural(value: Value) -> Result<Self> {
        let input: Self = serde_json::from_value(value)?;
        ensure!(
            (1..=4).contains(&input.questions.len()),
            "Expected 1-4 questions"
        );
        for q in &input.questions {
            ensure!((2..=4).contains(&q.options.len()), "Expected 2-4 options");
        }
        Ok(input)
    }
    /// TS zod refine（交互 broker 以拒绝收口）。
    pub fn refine(&self) -> Result<()> {
        self.refine_questions()?;
        self.refine_input()
    }
    /// 各题的 refine（zod 解析 `questions` 字段时报出，先于 annotations）。
    pub fn refine_questions(&self) -> Result<()> {
        let input = self;
        // 与 zod 遍历顺序一致、文案逐字取自 TS（docs/specs/rust-user-questions.md「入参 refine 失败」）：
        // 逐题先查各选项预览，再查本题选项重名与显式 Other；全部题目之后查问题重复。之前顺序与文案不同，
        // Node 回给模型的首条 issue 与 Rust 不一致。
        for q in &input.questions {
            for option in &q.options {
                if let Some(preview) = &option.preview {
                    validate_preview(preview)?;
                }
            }
            let labels: std::collections::BTreeSet<_> = q.options.iter().map(|o| &o.label).collect();
            ensure!(
                labels.len() == q.options.len(),
                "Option labels must be unique within each question"
            );
            ensure!(
                !q.options.iter().any(|o| o.label.trim().to_lowercase() == "other"),
                "Do not include an Other option; clients provide it automatically"
            );
        }
        Ok(())
    }
    /// 输入层 refine（对象其余字段都通过后才执行）。
    pub fn refine_input(&self) -> Result<()> {
        let questions: std::collections::BTreeSet<_> = self.questions.iter().map(|q| &q.question).collect();
        ensure!(
            questions.len() == self.questions.len(),
            "Question texts must be unique"
        );
        Ok(())
    }
    pub fn payload(&self, call_id: &str) -> Value {
        let questions = self.questions.iter().map(|q| json!({"question":q.question,"header":q.header,"multiSelect":q.multi_select,"options":q.options.iter().map(|o|{
            let mut option = json!({"value":o.label,"label":o.label,"description":o.description});
            if let Some(preview)=&o.preview { option["preview"]=preview.clone().into(); }
            option
        }).collect::<Vec<_>>()})).collect::<Vec<_>>();
        json!({"kind":"userInput","prompt":"AskUserQuestion pauses execution to collect answers from the user","freeText":true,"toolCallId":call_id,"toolName":"AskUserQuestion","input":self,"schema":{"toolName":"AskUserQuestion"},"questions":questions})
    }
}
/// annotations 各值的 zod 首条问题（TS `AskUserQuestionAnnotationSchema`，strict）。JSON Schema 校验不检查
/// `additionalProperties` 子 schema，这类问题在 TS 中由交互 broker 的 zod 解析报出（以拒绝收口）；之前 Rust 在
/// 反序列化时以工具错误结束，文案也不同。
pub fn annotation_issue(input: &Value) -> Option<String> {
    let kind = |v: &Value| match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    for value in input.get("annotations")?.as_object()?.values() {
        let Some(entry) = value.as_object() else {
            return Some(format!("Expected object, received {}", kind(value)));
        };
        for key in ["preview", "notes"] {
            if let Some(field) = entry.get(key).filter(|f| !f.is_string()) {
                return Some(format!("Expected string, received {}", kind(field)));
            }
        }
        let unknown: Vec<String> = entry
            .keys()
            .filter(|k| !matches!(k.as_str(), "preview" | "notes"))
            .map(|k| format!("'{k}'"))
            .collect();
        if !unknown.is_empty() {
            return Some(format!("Unrecognized key(s) in object: {}", unknown.join(", ")));
        }
    }
    None
}

fn validate_preview(preview: &str) -> Result<()> {
    static PATTERNS: OnceLock<[regex::Regex; 4]> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"(?i)<!doctype\b|<!--|</?\s*[a-z][a-z0-9:-]*(?:\s[^<>]*)?>",
            r"(?i)<!doctype\b|</?\s*(?:html|body)\b",
            r"(?i)</?\s*(?:script|style)\b",
            r"(?i)</?\s*[a-z][a-z0-9:-]*(?:\s[^<>]*)?>",
        ]
        .map(|p| regex::Regex::new(p).unwrap())
    });
    if patterns[0].is_match(preview) {
        ensure!(
            !patterns[1].is_match(preview),
            "HTML preview must be a fragment without html, body, or doctype"
        );
        ensure!(
            !patterns[2].is_match(preview),
            "HTML preview cannot contain script or style tags"
        );
        ensure!(
            patterns[3].is_match(preview),
            "HTML preview must contain an HTML tag"
        );
    }
    Ok(())
}

#[derive(Clone)]
pub struct QuestionAnswer {
    pub content: String,
    pub data: Value,
    pub failed: bool,
}

// Zod optional 允许缺省，但不接受 JSON null；serde Option 默认会吞掉 null，需显式保留差分。
pub(super) fn optional<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_issue_matches_zod_first_issue() {
        let issue = |v: Value| annotation_issue(&json!({ "annotations": v }));
        assert_eq!(issue(json!({"Q?": {"notes": null}})).as_deref(), Some("Expected string, received null"));
        assert_eq!(issue(json!({"Q?": "x"})).as_deref(), Some("Expected object, received string"));
        assert_eq!(
            issue(json!({"Q?": {"notes": "n", "a": 1, "b": 2}})).as_deref(),
            Some("Unrecognized key(s) in object: 'a', 'b'")
        );
        assert_eq!(issue(json!({"Q?": {"preview": "p", "notes": "n"}})), None);
        assert_eq!(annotation_issue(&json!({})), None);
    }
}
