//! XML 解析（spec §7.2 / §7.7）：元素层级、属性与节点路径（`/root/item[2]`）；按根的直接子元素分节。
//! 安全：quick-xml 不加载外部实体；含 `<!DOCTYPE` 内部实体声明的文档直接拒绝（防实体膨胀）；
//! 实体引用不展开，按原文保留。
use super::{Ctx, Failure, ParseResult, Parsed};
use escode_cli_domain::wiki::ir::{BlockSink, BlockType, locator};
use quick_xml::Reader;
use quick_xml::events::Event;

const MAX_TEXT_CHARS: usize = 300;

pub fn parse(text: &str, ctx: &Ctx<'_>) -> ParseResult {
    let mut reader = Reader::from_str(text);
    let mut parsed = Parsed::default();
    let mut sink = BlockSink::default();
    // 栈：(元素名, 路径, 同名兄弟计数表)
    let mut stack: Vec<(String, String, std::collections::HashMap<String, usize>)> = vec![];
    let mut root_counts: std::collections::HashMap<String, usize> = Default::default();
    let mut section: Option<(String, Vec<String>)> = None;
    let mut nodes = 0usize;
    // 当前元素累积的文本（Text / 实体 / CDATA 可能被拆成多个事件）。
    let mut pending = String::new();
    loop {
        let position = reader.buffer_position();
        let event = reader.read_event().map_err(|e| {
            let line = text[..(position as usize).min(text.len())]
                .matches('\n')
                .count()
                + 1;
            Failure::parse(format!("XML syntax error near line {line}: {e}"))
        })?;
        nodes += 1;
        if nodes > ctx.limits.max_nodes {
            return Err(Failure::limit(format!(
                "more than {} XML nodes",
                ctx.limits.max_nodes
            )));
        }
        if nodes.is_multiple_of(1000) {
            ctx.check_time()?;
        }
        // 自闭合元素没有 End 事件，不能入栈。
        let self_closing = matches!(event, Event::Empty(_));
        if matches!(
            event,
            Event::Start(_) | Event::Empty(_) | Event::End(_) | Event::Eof
        ) && !pending.trim().is_empty()
            && let (Some((_, lines)), Some((_, path, _))) = (section.as_mut(), stack.last())
        {
            let value: String = pending.trim().chars().take(MAX_TEXT_CHARS).collect();
            lines.push(format!("- `{path}`: {}", value.replace('\n', " ")));
        }
        if matches!(event, Event::Start(_) | Event::Empty(_) | Event::End(_)) {
            pending.clear();
        }
        match event {
            Event::DocType(doc) => {
                let decl = doc.to_string();
                if decl.contains("<!ENTITY") {
                    return Err(Failure::limit(
                        "XML declares DTD entities; refusing to expand them",
                    ));
                }
                parsed
                    .warnings
                    .push("DOCTYPE ignored (external DTDs are never loaded)".into());
            }
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name().as_ref().to_owned();
                let parent = stack.last().map_or(String::new(), |s| s.1.clone());
                let counts = stack
                    .last_mut()
                    .map(|s| &mut s.2)
                    .unwrap_or(&mut root_counts);
                let n = counts.entry(name.clone()).or_insert(0);
                *n += 1;
                let n = *n;
                let path = if n > 1 {
                    format!("{parent}/{name}[{n}]")
                } else {
                    format!("{parent}/{name}")
                };
                if stack.len() > ctx.limits.max_nesting_depth {
                    return Err(Failure::limit(format!(
                        "XML nesting deeper than {}",
                        ctx.limits.max_nesting_depth
                    )));
                }
                let attrs: Vec<String> = e
                    .attributes()
                    .flatten()
                    .map(|a| format!("{}=\"{}\"", a.key.as_ref(), a.value))
                    .collect();
                if stack.len() == 1 {
                    flush(&mut section, &mut sink);
                    section = Some((path.clone(), vec![]));
                }
                if let Some((_, lines)) = section.as_mut().filter(|_| !attrs.is_empty()) {
                    lines.push(format!("- `{path}` attributes: {}", attrs.join(" ")));
                }
                if stack.is_empty() {
                    parsed.title = Some(name.clone());
                }
                if !self_closing {
                    stack.push((name, path, Default::default()));
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Text(t) => pending.push_str(&t),
            Event::GeneralRef(r) => pending.push_str(&super::xmlutil::entity(&r)),
            Event::CData(t) => pending.push_str(&t),
            Event::PI(_) => parsed
                .warnings
                .push("processing instruction ignored".into()),
            Event::Eof => break,
            _ => {}
        }
    }
    flush(&mut section, &mut sink);
    parsed.blocks = sink.blocks;
    Ok(parsed)
}

fn flush(section: &mut Option<(String, Vec<String>)>, sink: &mut BlockSink) {
    if let Some((path, lines)) = section.take()
        && !lines.is_empty()
    {
        sink.push(
            BlockType::Field,
            std::slice::from_ref(&path),
            locator("path", path.clone()),
            lines.join("\n"),
        );
    }
}
