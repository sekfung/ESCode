//! 输入在模型边界附加的材料（TS `core/src/runtime/helpers/conversation-quotes.ts::withConversationQuotes`）：
//! 只包含对话引用。只进模型请求，不混入 canonical 用户正文或行投影的正文。
//! JSON 片段按 TS 对象字面量 / zod schema 的键序序列化，与 Node 逐字一致。

use crate::json_order::Json;
use serde_json::Value;

/// TS `withConversationQuotes(text, quotes)`；`quotes` 缺席时传 `Value::Null`。
pub fn with_conversation_material(text: &str, quotes: &Value) -> String {
    let mut parts: Vec<String> = vec![text.to_owned()];
    if let Some(quotes) = quotes.as_array().filter(|quotes| !quotes.is_empty()) {
        let items = quotes.iter().map(quote_json).collect();
        parts.push(format!(
            "<conversation_quote>\n{}\n</conversation_quote>",
            Json::Array(items).compact()
        ));
    }
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `conversationQuoteSchema` 的键序：text、path、senderName、senderId、messageId、sentAt。
fn quote_json(quote: &Value) -> Json {
    let mut item = Json::object();
    for key in [
        "text",
        "path",
        "senderName",
        "senderId",
        "messageId",
        "sentAt",
    ] {
        set_str(&mut item, key, &quote[key]);
    }
    item
}

fn set_str(target: &mut Json, key: &str, value: &Value) {
    if let Some(value) = value.as_str() {
        target.set(key, Json::str(value));
    }
}
