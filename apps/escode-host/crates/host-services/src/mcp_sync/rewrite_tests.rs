//! filesystem MCP 的远端路径改写（`rust-u2-sync.md` §2）。
use super::*;

fn paths<'a>() -> RewritePaths<'a> {
    RewritePaths {
        local_home: "/home/alice",
        local_workspace: Some("/home/alice/project"),
        remote_home: "/home/bob",
        remote_workspace: Some("/srv/project"),
    }
}

fn filesystem(args: Vec<&str>) -> Value {
    let mut all = vec!["-y", "@modelcontextprotocol/server-filesystem"];
    all.extend(args);
    serde_json::json!({"command": "npx", "args": all})
}

#[test]
fn workspace_and_home_prefixes_are_rewritten_to_the_remote_roots() {
    let config = filesystem(vec![
        "/home/alice/project/src",
        "/home/alice/data",
        "/opt/shared",
    ]);
    let rewritten = rewrite_filesystem_mcp_config("filesystem", &config, &paths());
    assert_eq!(
        rewritten["args"],
        serde_json::json!([
            "-y",
            "@modelcontextprotocol/server-filesystem",
            "/srv/project/src",
            "/home/bob/data",
            "/opt/shared"
        ])
    );
}

#[test]
fn only_stdio_filesystem_servers_are_touched() {
    let other = serde_json::json!({
        "command": "npx", "args": ["/home/alice/project"],
    });
    assert_eq!(
        rewrite_filesystem_mcp_config("github", &other, &paths()),
        other
    );

    let http = serde_json::json!({
        "type": "http", "command": "x", "args": ["/home/alice/project"],
    });
    assert_eq!(
        rewrite_filesystem_mcp_config("filesystem", &http, &paths()),
        http
    );

    let no_args = serde_json::json!({"command": "x"});
    assert_eq!(
        rewrite_filesystem_mcp_config("filesystem", &no_args, &paths()),
        no_args
    );

    // 名字不叫 filesystem，但命令里是 filesystem server。
    let by_command = serde_json::json!({
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/alice/x"],
    });
    let rewritten = rewrite_filesystem_mcp_config("docs", &by_command, &paths());
    assert_eq!(rewritten["args"][2], serde_json::json!("/home/bob/x"));
}

#[test]
fn windows_clients_rewrite_by_the_remote_path_shape() {
    let windows = RewritePaths {
        local_home: "C:\\Users\\alice",
        local_workspace: Some("C:\\Users\\alice\\project"),
        remote_home: "/home/bob",
        remote_workspace: Some("/srv/project"),
    };
    let config = filesystem(vec![
        "C:\\Users\\alice\\project\\src",
        "c:\\users\\ALICE\\data",
        "C:\\elsewhere",
    ]);
    let rewritten = rewrite_filesystem_mcp_config("fs", &config, &windows);
    assert_eq!(rewritten["args"][2], serde_json::json!("/srv/project/src"));
    assert_eq!(
        rewritten["args"][3],
        serde_json::json!("/home/bob/data"),
        "Windows 本机路径比较不区分大小写"
    );
    assert_eq!(rewritten["args"][4], serde_json::json!("C:\\elsewhere"));

    let to_windows = RewritePaths {
        local_home: "/home/alice",
        local_workspace: None,
        remote_home: "D:\\bob",
        remote_workspace: None,
    };
    let config = filesystem(vec!["/home/alice/data"]);
    let rewritten = rewrite_filesystem_mcp_config("filesystem", &config, &to_windows);
    assert_eq!(rewritten["args"][2], serde_json::json!("D:\\bob\\data"));
}

#[test]
fn root_remote_home_and_empty_relative_paths_are_handled() {
    let root = RewritePaths {
        local_home: "/home/alice",
        local_workspace: None,
        remote_home: "/",
        remote_workspace: None,
    };
    let config = filesystem(vec!["/home/alice/x"]);
    let rewritten = rewrite_filesystem_mcp_config("filesystem", &config, &root);
    assert_eq!(rewritten["args"][2], serde_json::json!("/x"));

    let same = RewritePaths {
        local_home: "/home/alice",
        local_workspace: None,
        remote_home: "/home/alice",
        remote_workspace: None,
    };
    let config = filesystem(vec!["/home/alice"]);
    let rewritten = rewrite_filesystem_mcp_config("filesystem", &config, &same);
    assert_eq!(rewritten["args"][2], serde_json::json!("/home/alice"));
}
