//! 退役 CUA MCP 判定（docs/specs/rust-browser-use.md「第 4 期细则」），对齐 TS `packages/shared/src/mcp.ts`
//! 的 escode-cua 包规格匹配与 `bootstrap/src/mcp-config.ts` 的 `isRetiredCuaMcpServer`。
use serde_json::Value;

pub const NODE_REPL: &str = "node_repl";
pub const OFFICIAL_PLUGIN_ID: &str = "computer-use@zcode-plugins-official";

fn leaf(value: &str) -> &str {
    // 先去掉结尾分隔符再取叶子，`.../escode-cua/` 不能得到空叶子而漏判（TS 注释：fail-closed）。
    let trimmed = value.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

fn matches_spec(candidate: &str) -> bool {
    let c = candidate.replace('_', "-");
    c == "escode-cua"
        || ["escode-cua[", "escode-cua@", "escode-cua==", "escode-cua."]
            .iter()
            .any(|p| c.starts_with(p))
}

// TS 以显式 type 为准；未写 type 但有 command 的配置在 Rust 解析中同为 stdio。
fn is_stdio(raw: &Value) -> bool {
    match raw["type"].as_str() {
        Some(kind) => kind == "stdio",
        None => raw["command"].is_string(),
    }
}

/// TS `isESCodeCuaMcpCommand` / `isESCodeCuaMcpPackageArg`（两者判定相同）。
pub fn is_cua_spec(value: &str) -> bool {
    matches_spec(value) || matches_spec(leaf(value))
}

/// 除 node_repl 外，CUA 形态的 stdio server 都已退役：不连接、不列出、不暴露工具。
pub fn is_retired(name: &str, raw: &Value) -> bool {
    if name == NODE_REPL {
        return false;
    }
    if !is_stdio(raw) {
        return false;
    }
    name == "computer-use"
        || raw["env"]["ESCODE_PLUGIN_ID"]
            .as_str()
            .is_some_and(|id| id.trim().to_lowercase() == OFFICIAL_PLUGIN_ID)
        || raw["command"].as_str().is_some_and(is_cua_spec)
        || raw["args"]
            .as_array()
            .is_some_and(|args| args.iter().filter_map(Value::as_str).any(is_cua_spec))
}

/// TS `computeOfficialCuaServerNames` 的最终门（`core/src/runtime/methods/mcp.ts`）：
/// server 名、tool 名和 manifest env 都可被第三方仿造，只有 resolver 写入的官方 plugin id 与宿主
/// 私有捕获的 authority 同时匹配才成立。authority 缺失或半组时 fail-closed。
/// `node_repl` 是共享宿主而非 CUA MCP server（其 CUA 结果走 node_repl 图片通道），这里显式排除，
/// 否则它注入的同一组 env 会被误当成官方 CUA，抢走 artifact 预算。
pub fn official_authority_verified(
    name: &str,
    raw: &Value,
    expected_authority: Option<&str>,
) -> bool {
    if name == NODE_REPL {
        return false;
    }
    let Some(expected) = expected_authority.map(str::trim).filter(|a| !a.is_empty()) else {
        return false;
    };
    is_stdio(raw)
        && raw["env"]["ESCODE_PLUGIN_ID"]
            .as_str()
            .is_some_and(|id| id.trim().to_lowercase() == OFFICIAL_PLUGIN_ID)
        && raw["env"]["ESCODE_CUA_PLUGIN_AUTHORITY"]
            .as_str()
            .is_some_and(|authority| authority.trim() == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_package_specs() {
        for spec in [
            "escode-cua",
            "escode_cua",
            "escode-cua[macos]",
            "escode-cua@1.2.3",
            "escode-cua==1.2.3",
            "escode-cua.git@v1",
            "escode_cua.server",
            "/opt/bin/escode-cua",
            r"C:\tools\escode-cua\",
            "git+https://example.invalid/org/escode-cua.git@v1",
        ] {
            assert!(is_cua_spec(spec), "{spec}");
        }
        for spec in ["escode-cua-proxy", "cua", "escode", ""] {
            assert!(!is_cua_spec(spec), "{spec}");
        }
    }

    #[test]
    fn retires_cua_shaped_stdio_servers_except_node_repl() {
        assert!(is_retired(
            "computer-use",
            &json!({"type": "stdio", "command": "x"})
        ));
        assert!(is_retired(
            "a",
            &json!({"command": "uvx", "args": ["--from", "escode-cua", "run"]})
        ));
        assert!(is_retired(
            "a",
            &json!({"type": "stdio", "command": "h", "env": {"ESCODE_PLUGIN_ID": " Computer-Use@zcode-plugins-official "}})
        ));
        assert!(!is_retired(
            NODE_REPL,
            &json!({"type": "stdio", "command": "escode-cua"})
        ));
        assert!(!is_retired(
            "computer-use",
            &json!({"type": "http", "url": "http://x"})
        ));
        assert!(!is_retired(
            "a",
            &json!({"type": "stdio", "command": "node", "args": ["server.js"]})
        ));
    }

    #[test]
    fn official_authority_requires_private_authority_and_official_plugin_id() {
        let raw = json!({"type": "stdio", "command": "x", "env": {
            "ESCODE_PLUGIN_ID": " Computer-Use@zcode-plugins-official ",
            "ESCODE_CUA_PLUGIN_AUTHORITY": " authority ",
        }});
        assert!(official_authority_verified(
            "plugin:escode-cua:computer-use",
            &raw,
            Some("authority")
        ));
        // authority 缺失/半组、错误 authority、错误 plugin id 都 fail-closed。
        assert!(!official_authority_verified("a", &raw, None));
        assert!(!official_authority_verified("a", &raw, Some(" ")));
        assert!(!official_authority_verified("a", &raw, Some("other")));
        let wrong_plugin = json!({"type": "stdio", "command": "x", "env": {
            "ESCODE_PLUGIN_ID": "third-party",
            "ESCODE_CUA_PLUGIN_AUTHORITY": "authority",
        }});
        assert!(!official_authority_verified(
            "a",
            &wrong_plugin,
            Some("authority")
        ));
        // 非 stdio 不适用；node_repl 注入同一组 env，但不是 CUA MCP server。
        let http = json!({"type": "http", "url": "http://x", "env": {
            "ESCODE_PLUGIN_ID": OFFICIAL_PLUGIN_ID,
            "ESCODE_CUA_PLUGIN_AUTHORITY": "authority",
        }});
        assert!(!official_authority_verified("a", &http, Some("authority")));
        assert!(!official_authority_verified(
            NODE_REPL,
            &raw,
            Some("authority")
        ));
    }
}
