//! YAML 解析（spec §7.7）：saphyr（YAML 1.2 core schema）解析，语法错误带行列；按顶层键分节、
//! 字段路径与类型同 JSON；顶层键前的注释保留进该节（saphyr 不保留注释，这里按行补回）。
//! 多文档只取第一个并告警；不解析自定义 tag 的语义（按内部值处理），不加载外部资源。
use super::json::sections;
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::frontmatter::yaml_to_value;
use escode_cli_domain::wiki::ir::BlockSink;
use saphyr::{LoadableYamlNode, YamlOwned};

pub fn parse(text: &str, ctx: &Ctx<'_>) -> ParseResult {
    let documents = YamlOwned::load_from_str(text)
        .map_err(|e| Failure::parse(format!("YAML syntax error: {e}")))?;
    let mut parsed = Parsed::default();
    if documents.len() > 1 {
        parsed.warnings.push(format!(
            "{} YAML documents found; only the first was imported",
            documents.len()
        ));
    }
    let Some(first) = documents.first() else {
        return Err(Failure::parse("YAML file contains no document"));
    };
    let value = yaml_to_value(first);
    let lines: Vec<&str> = text.lines().collect();
    let top_line = |key: &str| -> Option<usize> {
        lines.iter().position(|l| {
            !l.starts_with([' ', '\t', '#', '-'])
                && (l.starts_with(&format!("{key}:"))
                    || l.starts_with(&format!("\"{key}\":"))
                    || l.starts_with(&format!("'{key}':")))
        })
    };
    let line_of = |path: &str| top_line(path.strip_prefix("$.")?).map(|i| i + 1);
    let mut sink = BlockSink::default();
    sections(&value, ctx, &line_of, &mut sink)?;
    // 顶层键前紧邻的注释行并入该节正文。
    for block in &mut sink.blocks {
        let Some(index) = block
            .heading_path
            .first()
            .and_then(|p| p.strip_prefix("$."))
            .and_then(top_line)
        else {
            continue;
        };
        let mut comments = vec![];
        let mut i = index;
        while i > 0 && lines[i - 1].trim_start().starts_with('#') {
            comments.push(
                lines[i - 1]
                    .trim_start()
                    .trim_start_matches('#')
                    .trim()
                    .to_owned(),
            );
            i -= 1;
        }
        if !comments.is_empty() {
            comments.reverse();
            block.text = format!("Comments: {}\n{}", comments.join(" "), block.text);
        }
    }
    parsed
        .metadata
        .insert("rootType".into(), super::json::type_name(&value).into());
    parsed.blocks = sink.blocks;
    Ok(parsed)
}
