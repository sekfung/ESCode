use serde_json::Value;

/// 工具执行结果（模型可见内容、结构化数据、行展示与本轮控制）。
pub struct ToolOutput {
    pub failed: bool,
    pub content: String,
    pub data: Value,
    pub display: Option<Value>,
    pub control: ToolControl,
}
/// 工具结果对本轮的控制（TS ToolExecutionResult.turnControl / 拒绝投影）。
#[derive(Default, Clone, Copy)]
pub struct ToolControl {
    /// 按被拒收口：行 cancelled，不写输出（与权限拒绝一致）。
    pub denied: bool,
    /// 写入工具结果后结束本轮，不再请求模型。
    pub stop_turn: bool,
}
impl ToolOutput {
    pub fn text(content: String) -> Self {
        Self {
            failed: false,
            content,
            data: Value::Null,
            display: None,
            control: ToolControl::default(),
        }
    }
    pub fn new(content: String, data: Value) -> Self {
        Self {
            failed: false,
            content,
            data,
            display: None,
            control: ToolControl::default(),
        }
    }
}
