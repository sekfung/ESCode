//! 退役 CUA MCP 判定（docs/specs/rust-browser-use.md「第 4 期细则」），对齐 TS `packages/shared/src/mcp.ts`
//! 的 zcode-cua 包规格匹配与 `bootstrap/src/mcp-config.ts` 的 `isRetiredCuaMcpServer`。
use serde_json::Value;

pub const NODE_REPL: &str = "node_repl";
pub const OFFICIAL_PLUGIN_ID: &str = "computer-use@zcode-plugins-official";

fn leaf(value: &str) -> &str {
    // 先去掉结尾分隔符再取叶子，`.../zcode-cua/` 不能得到空叶子而漏判（TS 注释：fail-closed）。
    let trimmed = value.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

fn matches_spec(candidate: &str) -> bool {
    let c = candidate.replace('_', "-");
    c == "zcode-cua" || ["zcode-cua[", "zcode-cua@", "zcode-cua==", "zcode-cua."].iter().any(|p| c.starts_with(p))
}

/// TS `isZCodeCuaMcpCommand` / `isZCodeCuaMcpPackageArg`（两者判定相同）。
pub fn is_cua_spec(value: &str) -> bool {
    matches_spec(value) || matches_spec(leaf(value))
}

/// 除 node_repl 外，CUA 形态的 stdio server 都已退役：不连接、不列出、不暴露工具。
pub fn is_retired(name: &str, raw: &Value) -> bool {
    if name == NODE_REPL {
        return false;
    }
    // TS 以显式 type 为准；未写 type 但有 command 的配置在 Rust 解析中同为 stdio。
    let stdio = match raw["type"].as_str() {
        Some(kind) => kind == "stdio",
        None => raw["command"].is_string(),
    };
    if !stdio {
        return false;
    }
    name == "computer-use"
        || raw["env"]["ZCODE_PLUGIN_ID"]
            .as_str()
            .is_some_and(|id| id.trim().to_lowercase() == OFFICIAL_PLUGIN_ID)
        || raw["command"].as_str().is_some_and(is_cua_spec)
        || raw["args"]
            .as_array()
            .is_some_and(|args| args.iter().filter_map(Value::as_str).any(is_cua_spec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_package_specs() {
        for spec in [
            "zcode-cua",
            "zcode_cua",
            "zcode-cua[macos]",
            "zcode-cua@1.2.3",
            "zcode-cua==1.2.3",
            "zcode-cua.git@v1",
            "zcode_cua.server",
            "/opt/bin/zcode-cua",
            r"C:\tools\zcode-cua\",
            "git+https://example.invalid/org/zcode-cua.git@v1",
        ] {
            assert!(is_cua_spec(spec), "{spec}");
        }
        for spec in ["zcode-cua-proxy", "cua", "zcode", ""] {
            assert!(!is_cua_spec(spec), "{spec}");
        }
    }

    #[test]
    fn retires_cua_shaped_stdio_servers_except_node_repl() {
        assert!(is_retired("computer-use", &json!({"type": "stdio", "command": "x"})));
        assert!(is_retired("a", &json!({"command": "uvx", "args": ["--from", "zcode-cua", "run"]})));
        assert!(is_retired("a", &json!({"type": "stdio", "command": "h", "env": {"ZCODE_PLUGIN_ID": " Computer-Use@zcode-plugins-official "}})));
        assert!(!is_retired(NODE_REPL, &json!({"type": "stdio", "command": "zcode-cua"})));
        assert!(!is_retired("computer-use", &json!({"type": "http", "url": "http://x"})));
        assert!(!is_retired("a", &json!({"type": "stdio", "command": "node", "args": ["server.js"]})));
    }
}
