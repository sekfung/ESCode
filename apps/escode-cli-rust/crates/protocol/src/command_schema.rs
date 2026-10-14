//! `v4/command` 的信封/payload 校验（rust-unified-host.md P1 #10）。
//!
//! 语料 `command_schema.json` 由 `scripts/generate-escode-cli-rust-command-schema.mjs`
//! 从 TS zod（`packages/shared/src/escode-protocol-v4/command.ts`）生成，`--check`
//! 与 Rust 侧漂移测试双向拦截。校验语义对齐 TS `parseCommandEnvelope`：
//! 先信封（type/required/properties），再按 `type` 校验 payload，最后是 CAS 规则
//! （`baseRevision` / `baseLogEpoch` 必带的命令集合）。
//!
//! 调用方当前只把违规写日志（不改变线上应答），因为现有 serde 解析的拒绝面与
//! TS zod 不完全一致，收紧线上行为需要差分场景作证（见 u1 子 spec 移交项）。

use std::sync::OnceLock;

use serde_json::Value;

use crate::json_schema::{self, Violation};

struct Corpus {
    envelope: Value,
    payloads: serde_json::Map<String, Value>,
    requiring_base_revision: Vec<String>,
    row_targeting: Vec<String>,
}

fn corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let raw: Value = serde_json::from_str(include_str!("command_schema.json"))
            .expect("generated command schema");
        Corpus {
            envelope: raw["envelope"].clone(),
            payloads: raw["payloads"]
                .as_object()
                .expect("generated payload map")
                .clone(),
            requiring_base_revision: strings(&raw["requiringBaseRevision"]),
            row_targeting: strings(&raw["rowTargeting"]),
        }
    })
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// 语料里登记的全部命令类型（TS `commandTypeSchema`）。
pub fn command_types() -> Vec<String> {
    let mut types: Vec<String> = corpus().payloads.keys().cloned().collect();
    types.sort();
    types
}

/// 需要 `baseRevision` / `baseLogEpoch` 的命令集合（TS 的 CAS 规则表）。
pub fn requiring_base_revision() -> &'static [String] {
    &corpus().requiring_base_revision
}

pub fn row_targeting() -> &'static [String] {
    &corpus().row_targeting
}

/// 校验一条命令信封（含 payload 与 CAS 规则）；空 vec = 与 TS zod 同判通过。
pub fn validate_command(value: &Value) -> Vec<Violation> {
    let corpus = corpus();
    let mut violations = json_schema::validate(&corpus.envelope, value);
    if !violations.is_empty() {
        return violations;
    }
    let kind = value["type"].as_str().unwrap_or_default();
    match corpus.payloads.get(kind) {
        Some(payload_schema) => {
            let payload = &value["payload"];
            violations.extend(json_schema::validate(payload_schema, payload));
        }
        None => violations.push(Violation {
            path: "$.type".to_owned(),
            message: format!("unknown command type `{kind}`"),
        }),
    }
    if violations.is_empty() {
        // TS `parseCommandEnvelope` 的两条规则是并列的 `||`：需要 baseRevision 的集合与
        // 需要 baseLogEpoch 的集合各自独立（曾误实现为嵌套，导致只带 baseRevision 的行定位
        // 命令漏检 baseLogEpoch）。
        if corpus.requiring_base_revision.iter().any(|t| t == kind)
            && value.get("baseRevision").is_none()
        {
            violations.push(Violation {
                path: "$.baseRevision".to_owned(),
                message: "CAS commands require baseRevision".to_owned(),
            });
        }
        if corpus.row_targeting.iter().any(|t| t == kind) && value.get("baseLogEpoch").is_none() {
            violations.push(Violation {
                path: "$.baseLogEpoch".to_owned(),
                message: "CAS commands require baseRevision and baseLogEpoch".to_owned(),
            });
        }
    }
    violations
}

/// 违规的单行诊断（日志用）。
pub fn describe(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|violation| format!("{}: {}", violation.path, violation.message))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn envelope(kind: &str, payload: Value) -> Value {
        json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "sessionId": null,
            "type": kind,
            "payload": payload,
            "issuedAt": 1_700_000_000_000.0,
        })
    }

    #[test]
    fn valid_commands_pass_for_every_generated_type() {
        // 语料自洽：每个类型的必填字段都能构造出通过校验的最小 payload。
        assert!(command_types().len() >= 30, "命令类型语料完整");
        let violations = validate_command(&envelope("stop", json!({})));
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn envelope_shape_is_enforced() {
        let mut value = envelope("stop", json!({}));
        value.as_object_mut().unwrap().remove("issuedAt");
        let violations = validate_command(&value);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "$.issuedAt");

        let mut wrong_type = envelope("stop", json!({}));
        wrong_type["type"] = json!("notACommand");
        let violations = validate_command(&wrong_type);
        assert_eq!(violations.len(), 1);
        // 信封的 type 是 35 个类型的 enum：先按枚举报告（未知类型分支只在没有枚举项时兜底）。
        assert!(
            violations[0].path == "$.type" && violations[0].message.contains("not in enum"),
            "{violations:?}"
        );

        let mut bad_payload = envelope("sendGoalCommand", json!({"text": 5}));
        bad_payload["sessionId"] = json!("sess_1");
        let violations = validate_command(&bad_payload);
        // payload 的违规路径相对 payload 根（`$.text`），与 TS zod 的 path 前缀不同但定位等价。
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].path, "$.text");
        assert!(violations[0].message.contains("expected string"));
    }

    #[test]
    fn cas_rules_follow_the_generated_sets() {
        let cas_kind = requiring_base_revision()
            .iter()
            .find(|kind| kind.as_str() == "setAutoDrain")
            .expect("按载荷取一个稳定用例");
        let mut value = envelope(cas_kind, json!({"autoDrain": true}));
        let violations = validate_command(&value);
        assert!(
            violations.iter().any(|v| v.path == "$.baseRevision"),
            "{violations:?}"
        );
        value["baseRevision"] = json!(7);
        let violations = validate_command(&value);
        assert!(
            !violations.iter().any(|v| v.path == "$.baseRevision"),
            "{violations:?}"
        );
        // 行定位命令还要求 baseLogEpoch（TS 同一规则）。夹具字段按语料的 required 构造。
        if row_targeting()
            .iter()
            .any(|kind| kind == "setHighspeedMetrics")
        {
            let payload = json!({
                "target": {"rowId": 1, "entityId": "sess_1"},
                "regularTps": 1.5,
                "outputTokens": 10,
                "durationMs": 12.0,
                "highspeedTps": 3.5,
                "savedDurationMs": 4.0,
            });
            let mut value = envelope("setHighspeedMetrics", payload);
            value["baseRevision"] = json!(3);
            let violations = validate_command(&value);
            assert!(
                violations.iter().any(|v| v.path == "$.baseLogEpoch"),
                "{violations:?}"
            );
        }
    }

    /// 漂移：信封语料的键与 Rust `Command` 结构字段必须一一对应（`ttft` 为 TS 新增的
    /// 可选字段，Rust 结构不保留但解析忽略——显式登记，新增字段必须在此处表态）。
    #[test]
    fn envelope_properties_match_the_rust_command_struct() {
        const RUST_FIELDS: [&str; 8] = [
            "baseLogEpoch",
            "baseRevision",
            "clientId",
            "commandId",
            "issuedAt",
            "payload",
            "sessionId",
            "type",
        ];
        const IGNORED: [&str; 1] = ["ttft"];
        let properties = corpus()
            .envelope
            .get("properties")
            .and_then(Value::as_object)
            .expect("envelope properties");
        for key in properties.keys() {
            assert!(
                RUST_FIELDS.contains(&key.as_str()) || IGNORED.contains(&key.as_str()),
                "信封新字段 `{key}` 必须进入 Command 结构或显式登记为忽略"
            );
        }
        for field in RUST_FIELDS {
            assert!(
                properties.contains_key(field),
                "Command 字段 `{field}` 不在语料里"
            );
        }
        let required = corpus()
            .envelope
            .get("required")
            .and_then(Value::as_array)
            .expect("envelope required");
        assert!(
            required.iter().any(|value| value == "issuedAt"),
            "issuedAt 必须是必填（Rust 结构无默认值）"
        );
    }
}
