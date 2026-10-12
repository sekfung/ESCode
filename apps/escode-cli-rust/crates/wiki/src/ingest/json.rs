//! JSON 解析（spec §7.7）：语法错误报告行列；按顶层键分节（数组按元素），每节列出叶子字段路径
//! （`$.a.b[0]`）、类型与值；限制嵌套深度与节点数。
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};
use serde_json::Value;

const MAX_VALUE_CHARS: usize = 200;

pub fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn child_path(parent: &str, key: &str) -> String {
    if key.chars().all(|c| c.is_alphanumeric() || c == '_') && !key.is_empty() {
        format!("{parent}.{key}")
    } else {
        format!(
            "{parent}[{}]",
            serde_json::to_string(key).unwrap_or_default()
        )
    }
}

/// 叶子行：`path (type): value`；同时检查深度与节点数上限。
pub fn leaves(
    value: &Value,
    path: &str,
    depth: usize,
    ctx: &Ctx<'_>,
    nodes: &mut usize,
    out: &mut Vec<String>,
) -> Result<(), Failure> {
    *nodes += 1;
    if depth > ctx.limits.max_nesting_depth {
        return Err(Failure::limit(format!(
            "nesting deeper than {} at {path}",
            ctx.limits.max_nesting_depth
        )));
    }
    if *nodes > ctx.limits.max_nodes {
        return Err(Failure::limit(format!(
            "more than {} nodes",
            ctx.limits.max_nodes
        )));
    }
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                leaves(child, &child_path(path, key), depth + 1, ctx, nodes, out)?;
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (i, child) in items.iter().enumerate() {
                leaves(child, &format!("{path}[{i}]"), depth + 1, ctx, nodes, out)?;
            }
        }
        other => {
            let text = match other {
                Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            let text: String = text.chars().take(MAX_VALUE_CHARS).collect();
            out.push(format!(
                "- `{path}` ({}): {}",
                type_name(other),
                text.replace('\n', " ")
            ));
        }
    }
    Ok(())
}

/// 结构化值 → 分节块（JSON 与 YAML 共用）。`line_of(path)` 给出该节在原文的行号（可缺）。
pub fn sections(
    root: &Value,
    ctx: &Ctx<'_>,
    line_of: &dyn Fn(&str) -> Option<usize>,
    sink: &mut BlockSink,
) -> Result<(), Failure> {
    let mut nodes = 0;
    let children: Vec<(String, &Value)> = match root {
        Value::Object(map) => map.iter().map(|(k, v)| (child_path("$", k), v)).collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("$[{i}]"), v))
            .collect(),
        other => vec![("$".to_owned(), other)],
    };
    for (path, value) in children {
        ctx.check_time()?;
        let mut out = vec![];
        leaves(value, &path, 1, ctx, &mut nodes, &mut out)?;
        let loc = match line_of(&path) {
            Some(line) => format!("{path} (line {line})"),
            None => path.clone(),
        };
        sink.push(
            BlockType::Field,
            std::slice::from_ref(&path),
            locator("path", loc),
            out.join("\n"),
        );
    }
    Ok(())
}

pub fn parse(text: &str, ctx: &Ctx<'_>) -> ParseResult {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        Failure::parse(format!(
            "JSON syntax error at line {}, column {}: {e}",
            e.line(),
            e.column()
        ))
    })?;
    let mut sink = BlockSink::default();
    let line_of = |path: &str| -> Option<usize> {
        // 顶层键所在行：查找 `"key"` 后跟冒号的第一行（只用于定位展示）。
        let key = path.strip_prefix("$.")?;
        let needle = format!("\"{key}\"");
        text.lines()
            .position(|l| l.trim_start().starts_with(&needle))
            .map(|i| i + 1)
    };
    sections(&value, ctx, &line_of, &mut sink)?;
    let mut parsed = Parsed {
        blocks: sink.blocks,
        ..Default::default()
    };
    parsed
        .metadata
        .insert("rootType".into(), type_name(&value).into());
    Ok(parsed)
}
