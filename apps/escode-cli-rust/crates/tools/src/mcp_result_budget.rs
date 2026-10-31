use super::mcp_config::Server;
use super::mcp_image_artifacts::ImageArtifacts;
use serde_json::Value;

/// MCP 文本结果的模型预算（docs/specs/rust-mcp-parity.md「大结果预算」）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum BudgetKind {
    /// 保留开头（或回退时保留尾部）+ suffix，对应 Node `strategy=truncate`。
    Truncate,
    /// 完整文本写 artifact，模型只看到 `<persisted-output>` 信封（Node `strategy=artifact`）。
    Artifact,
}

/// Node 普通 MCP 走通用 `resultBudget`（50_000 字节、head、`strategy=truncate`）；
/// `node_repl` 的 `js` 走 artifact 预算（64 KiB 触发、session retention）；只有通过
/// `mcp_cua::official_authority_verified` 的官方 CUA server 才用 256 KiB `truncate`。
/// 未验证的 CUA 形 server 保留既有 64 KiB 行为，不按名字放宽。
#[derive(Clone, Copy)]
pub(super) struct ResultBudget {
    max_model_bytes: usize,
    kind: BudgetKind,
    node_format: bool,
}

impl ResultBudget {
    pub(super) fn for_server(server: &Server) -> Self {
        // 与 TS 一样只信任宿主启动时捕获的私有 authority；cua_credentials 要求 socket 与 authority
        // 成组，缺失/半组返回 None，从而 fail-closed 到普通或遗留预算。
        let authority = escode_cli_host::child_env::cua_credentials().map(|c| c.authority);
        Self::resolve(&server.name, &server.raw, authority.as_deref())
    }

    pub(super) fn resolve(name: &str, raw: &Value, expected_authority: Option<&str>) -> Self {
        if name == super::mcp_node_repl::NAME {
            return Self {
                max_model_bytes: 64 * 1024,
                kind: BudgetKind::Artifact,
                node_format: true,
            };
        }
        if crate::domain::mcp_cua::official_authority_verified(name, raw, expected_authority) {
            return Self {
                max_model_bytes: 256 * 1024,
                kind: BudgetKind::Truncate,
                node_format: true,
            };
        }
        let special = name == "escode-cua"
            || name == "computer-use"
            || (name.contains("escode-cua") && name.contains("computer-use"));
        if special {
            return Self {
                max_model_bytes: 64 * 1024,
                kind: BudgetKind::Truncate,
                node_format: false,
            };
        }
        Self {
            max_model_bytes: 50_000,
            kind: BudgetKind::Truncate,
            node_format: true,
        }
    }

    pub(super) async fn apply(
        self,
        content: &str,
        artifacts: Option<ImageArtifacts<'_>>,
    ) -> Option<String> {
        if content.len() <= self.max_model_bytes {
            return None;
        }
        if self.kind == BudgetKind::Artifact
            && let Some(target) = artifacts
            // TS tryWriteToolArtifact：artifact 写入失败不应把一次成功的工具调用变成失败，
            // 这里与 Node 一样回退到截断路径（MCP 的结果对象内容类型是 application/json）。
            && let Ok(path) = target.write_text("application/json", content).await
        {
            return Some(crate::domain::mcp_result::persisted_output_envelope(
                content,
                content.len(),
                &path,
            ));
        }
        let mut text = content.to_owned();
        if !self.node_format {
            super::tools::truncate_utf8(&mut text, self.max_model_bytes);
            text.push_str("\n[MCP result truncated]");
            return Some(text);
        }
        let strategy = if self.kind == BudgetKind::Artifact {
            "artifact"
        } else {
            "truncate"
        };
        // Node fitContentWithSuffix：先给 suffix 留位，再按 UTF-8 字符边界截取。
        let suffix = format!(
            "\n\n[Tool output truncated by resultBudget: originalBytes={}, maxModelBytes={}, strategy={strategy}]",
            content.len(),
            self.max_model_bytes
        );
        let remaining = self.max_model_bytes.saturating_sub(suffix.len());
        if remaining == 0 {
            return Some(suffix);
        }
        if self.kind == BudgetKind::Artifact {
            super::tools::truncate_utf8_tail(&mut text, remaining);
        } else {
            super::tools::truncate_utf8(&mut text, remaining);
        }
        text.push_str(&suffix);
        Some(text)
    }
}
