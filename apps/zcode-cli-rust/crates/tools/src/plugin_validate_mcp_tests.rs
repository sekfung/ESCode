//! 插件 MCP 声明校验的模板 / 服务器规则单测（文案逐字对齐 TS plugins/mcp.ts）。
use super::plugin_validate_mcp::*;
use crate::domain::json_order::Json;

fn context(manifest: &Json) -> VariableContext<'_> {
    VariableContext {
        manifest,
        defaults: vec![("region".into(), Json::str("us"))],
    }
}

#[test]
fn template_rules() {
    let manifest = Json::parse(r#"{"userConfig":{"token":{"sensitive":true}}}"#).unwrap();
    let ctx = context(&manifest);
    assert!(template("${CLAUDE_PLUGIN_ROOT}/x ${user_config.region}", &ctx, false).is_ok());
    assert!(template("${HOME}", &ctx, false).is_ok());
    assert_eq!(
        template("${HOME}", &ctx, true),
        Err((true, "Missing environment variable: HOME".into()))
    );
    assert_eq!(
        template("${user_config.token}", &ctx, false),
        Err((
            true,
            "Sensitive plugin user_config value cannot be used in this field: token".into()
        ))
    );
    assert_eq!(
        template("${user_config.token}", &ctx, true),
        Err((true, "Missing plugin user_config value: token".into()))
    );
    assert_eq!(
        template("${ZCODE_SESSION_ID}", &ctx, false),
        Err((
            true,
            "Plugin variable requires a runtime session context: ZCODE_SESSION_ID".into()
        ))
    );
}

#[test]
fn server_rules() {
    let manifest = Json::object();
    let ctx = context(&manifest);
    let check = |text: &str| check_server("s", &Json::parse(text).unwrap(), &ctx);
    assert!(check(r#"{"command":"node","args":["${CLAUDE_PLUGIN_ROOT}/a.js"]}"#).is_ok());
    assert_eq!(
        check(r#"{"type":"ws","url":"x"}"#),
        Err((false, "Unsupported MCP transport: ws".into()))
    );
    assert_eq!(
        check(r#"{"type":"sse"}"#),
        Err((false, "sse MCP server requires url".into()))
    );
    assert_eq!(
        check(r#"{"url":"https://x","oauth":{"type":"magic"}}"#),
        Err((false, "Unsupported MCP OAuth type: magic".into()))
    );
}
