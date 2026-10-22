//! svn `--xml` 输出的最小解析器（TS `repo/svnXml.ts`）。
//!
//! svn 输出只含元素、属性、文本和标准实体，不含 DTD / CDATA / 命名空间；
//! 这里只实现这一子集，不引入 XML 依赖。

use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XmlNode {
    pub name: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<XmlNode>,
    pub text: String,
}

struct OpenTag {
    name: String,
    attrs: BTreeMap<String, String>,
    self_closing: bool,
}

struct Parser<'a> {
    xml: &'a str,
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn skip_prolog(&mut self) {
        loop {
            let trimmed = self.xml[self.cursor..].trim_start();
            self.cursor = self.xml.len() - trimmed.len();
            if let Some(rest) = self.xml[self.cursor..].strip_prefix("<?") {
                self.cursor = rest
                    .find("?>")
                    .map_or(self.xml.len(), |end| self.cursor + 2 + end + 2);
                continue;
            }
            if let Some(rest) = self.xml[self.cursor..].strip_prefix("<!--") {
                self.cursor = rest
                    .find("-->")
                    .map_or(self.xml.len(), |end| self.cursor + 4 + end + 3);
                continue;
            }
            break;
        }
    }

    fn read_open_tag(&mut self) -> Option<OpenTag> {
        if !self.xml[self.cursor..].starts_with('<') {
            return None;
        }
        let end = find_tag_end(self.xml, self.cursor + 1)?;
        let tag = &self.xml[self.cursor + 1..end];
        self.cursor = end + 1;
        let self_closing = tag.trim_end().ends_with('/');
        let body = if self_closing {
            tag.trim_end().trim_end_matches('/').trim_end()
        } else {
            tag
        };
        let name_end = body
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(index, _)| index)
            .unwrap_or(body.len());
        Some(OpenTag {
            name: body[..name_end].to_owned(),
            attrs: parse_attributes(&body[name_end..]),
            self_closing,
        })
    }

    fn parse_element(&mut self) -> Option<XmlNode> {
        let tag = self.read_open_tag()?;
        let mut node = XmlNode {
            name: tag.name,
            attrs: tag.attrs,
            children: Vec::new(),
            text: String::new(),
        };
        if tag.self_closing {
            return Some(node);
        }
        loop {
            let Some(relative) = self.xml[self.cursor..].find('<') else {
                node.text
                    .push_str(&decode_entities(&self.xml[self.cursor..]));
                self.cursor = self.xml.len();
                return Some(node);
            };
            let start = self.cursor + relative;
            if start > self.cursor {
                node.text
                    .push_str(&decode_entities(&self.xml[self.cursor..start]));
            }
            if self.xml[start..].starts_with("</") {
                let end = self.xml[start..].find('>')? + start;
                self.cursor = end + 1;
                return Some(node);
            }
            if self.xml[start..].starts_with("<?") {
                self.cursor = match self.xml[start..].find("?>") {
                    Some(end) => start + end + 2,
                    None => self.xml.len(),
                };
                continue;
            }
            if self.xml[start..].starts_with("<!--") {
                self.cursor = match self.xml[start..].find("-->") {
                    Some(end) => start + end + 3,
                    None => self.xml.len(),
                };
                continue;
            }
            self.cursor = start;
            let child = self.parse_element()?;
            node.children.push(child);
        }
    }
}

pub fn parse(xml: &str) -> Option<XmlNode> {
    let mut parser = Parser { xml, cursor: 0 };
    parser.skip_prolog();
    parser.parse_element()
}

/// 找到标签结束的 `>`，跳过属性值引号里的 `>`。
fn find_tag_end(xml: &str, from: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (offset, ch) in xml[from..].char_indices() {
        match (quote, ch) {
            (Some(current), _) if ch == current => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '>') => return Some(from + offset),
            (None, _) => {}
        }
    }
    None
}

fn parse_attributes(source: &str) -> BTreeMap<String, String> {
    let mut attrs = BTreeMap::new();
    let bytes = source.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'='
            && bytes[index] != b'/'
        {
            index += 1;
        }
        let name = &source[name_start..index];
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            break;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || (bytes[index] != b'"' && bytes[index] != b'\'') {
            break;
        }
        let quote = bytes[index];
        index += 1;
        let value_start = index;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        attrs.insert(
            name.to_owned(),
            decode_entities(&source[value_start..index]),
        );
        index += 1;
    }
    attrs
}

fn decode_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0usize;
    while let Some(relative) = value[cursor..].find('&') {
        let start = cursor + relative;
        let Some(end_relative) = value[start..].find(';') else {
            break;
        };
        let end = start + end_relative;
        let entity = &value[start + 1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => decode_numeric(entity),
        };
        match decoded {
            Some(ch) => {
                output.push_str(&value[cursor..start]);
                output.push(ch);
                cursor = end + 1;
            }
            None => {
                output.push_str(&value[cursor..=end]);
                cursor = end + 1;
            }
        }
    }
    output.push_str(&value[cursor..]);
    output
}

fn decode_numeric(entity: &str) -> Option<char> {
    let (digits, radix) = if let Some(hex) = entity.strip_prefix("#x") {
        (hex, 16)
    } else if let Some(decimal) = entity.strip_prefix('#') {
        (decimal, 10)
    } else {
        return None;
    };
    u32::from_str_radix(digits, radix)
        .ok()
        .and_then(char::from_u32)
}

pub fn child<'a>(node: Option<&'a XmlNode>, name: &str) -> Option<&'a XmlNode> {
    node?.children.iter().find(|child| child.name == name)
}

pub fn children<'a>(node: Option<&'a XmlNode>, name: &str) -> Vec<&'a XmlNode> {
    node.map_or_else(Vec::new, |node| {
        node.children
            .iter()
            .filter(|child| child.name == name)
            .collect()
    })
}

pub fn path<'a>(node: Option<&'a XmlNode>, names: &[&str]) -> Option<&'a XmlNode> {
    let mut current = node;
    for name in names {
        current = child(current, name);
    }
    current
}

pub fn text(node: Option<&XmlNode>) -> Option<String> {
    node.map(|node| node.text.clone())
}

pub fn int_attr(node: Option<&XmlNode>, name: &str) -> Option<i64> {
    node?.attrs.get(name)?.parse().ok()
}
