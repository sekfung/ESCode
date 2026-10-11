//! ReplyToChannel（v3.15.1）：向当前机器人会话追加带原生 @ 的回复，经会话 owner 发 Host 反向请求
//! `bot/channel/reply`。对齐 TS `tool/handlers/reply-to-channel.ts`，见 docs/specs/rust-v3.15.1-drift.md。

use crate::contract::{Event, EventSink, ToolOutput};
use crate::domain::json_order::Json;
use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const METHOD: &str = "bot/channel/reply";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub(super) async fn execute(
    args: &Value,
    facts: &super::context::TurnFacts,
    call_id: &str,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Result<ToolOutput> {
    let parts = parse_parts(args)?;
    // TS 以执行该工具的 assistant 消息的父 user 消息的 sourceCommandId 作 inputId；没有渠道输入身份时不重试。
    let Some(input_id) = facts.source_command_id.clone() else {
        bail!("Channel reply execution input unavailable; retrying will not restore it");
    };
    let params = json!({
        "taskId": sink.session_id,
        "inputId": input_id,
        "toolCallId": call_id,
        "parts": parts,
    });
    let (reply, answer) = oneshot::channel();
    sink.send(Event::HostRequest {
        method: METHOD.into(),
        params,
        reply,
    })
    .await?;
    let raw = tokio::select! {biased;
        _ = cancel.cancelled() => bail!("Cancelled"),
        _ = tokio::time::sleep(TIMEOUT) => bail!("Channel reply timed out"),
        answer = answer => match answer {
            Ok(Ok(raw)) => raw,
            // Host 未实现该方法等价于 TS 端口缺席。
            Ok(Err((-32601, _, _))) => bail!("Channel reply is unavailable"),
            Ok(Err((_, message, _))) => bail!(message),
            Err(_) => bail!("Session owner stopped before the Host replied"),
        },
    };
    let result = Json::parse(&raw)
        .filter(Json::is_object)
        .ok_or_else(|| anyhow!("Invalid Host response"))?;
    // TS 对对象结果 JSON.stringify：保持 Host 返回的键序。
    Ok(ToolOutput::new(
        result.compact(),
        serde_json::from_str(&raw).unwrap_or(Value::Null),
    ))
}

/// TS channelReplyPartsSchema：1..=50 段，text / mention / mentionName 三种严格对象；mentionName.name 先 trim。
fn parse_parts(args: &Value) -> Result<Vec<Value>> {
    let invalid = |detail: &str| anyhow!("Invalid ReplyToChannel input: {detail}");
    let object = args.as_object().ok_or_else(|| invalid("expected object"))?;
    if object.keys().any(|key| key != "parts") {
        return Err(invalid("unrecognized key"));
    }
    let parts = object
        .get("parts")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("parts must be an array"))?;
    if parts.is_empty() || parts.len() > 50 {
        return Err(invalid("parts must contain 1 to 50 items"));
    }
    parts
        .iter()
        .map(|part| parse_part(part).ok_or_else(|| invalid("invalid part")))
        .collect()
}

fn parse_part(part: &Value) -> Option<Value> {
    let object = part.as_object()?;
    // zod 的字符串长度按 UTF-16 码元计。
    let text = |key: &str, min: usize, max: usize| {
        let value = object.get(key)?.as_str()?;
        let len = value.encode_utf16().count();
        (min..=max).contains(&len).then_some(value)
    };
    let only = |keys: &[&str]| object.keys().all(|key| keys.contains(&key.as_str()));
    match object.get("type")?.as_str()? {
        "text" if only(&["type", "text"]) => {
            text("text", 0, 2000)?;
            Some(part.clone())
        }
        "mention" if only(&["type", "refId"]) => {
            text("refId", 1, 200)?;
            Some(part.clone())
        }
        "mentionName" if only(&["type", "name", "candidateRef"]) => {
            let name = object.get("name")?.as_str()?.trim();
            let len = name.encode_utf16().count();
            if !(1..=200).contains(&len) {
                return None;
            }
            let mut out = Map::new();
            out.insert("type".into(), "mentionName".into());
            out.insert("name".into(), name.into());
            if object.contains_key("candidateRef") {
                out.insert("candidateRef".into(), text("candidateRef", 1, 200)?.into());
            }
            Some(Value::Object(out))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_parts;
    use serde_json::json;

    #[test]
    fn parts_follow_the_ts_strict_union() {
        let parts = parse_parts(&json!({"parts":[
            {"type":"text","text":"hi"},
            {"type":"mention","refId":"m1"},
            {"type":"mentionName","name":"  Alex Smith ","candidateRef":"c1"}
        ]}))
        .unwrap();
        assert_eq!(
            parts[2],
            json!({"type":"mentionName","name":"Alex Smith","candidateRef":"c1"})
        );
        for bad in [
            json!({"parts":[]}),
            json!({"parts":[{"type":"text","text":"x","refId":"y"}]}),
            json!({"parts":[{"type":"mention","refId":""}]}),
            json!({"parts":[{"type":"mentionName","name":"   "}]}),
            json!({"parts":[{"type":"text","text":"a".repeat(2001)}]}),
            json!({"parts":[{"type":"text","text":"x"}],"extra":1}),
        ] {
            assert!(parse_parts(&bad).is_err(), "{bad}");
        }
    }
}
