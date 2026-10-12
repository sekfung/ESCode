//! quick-xml 0.42 的文本辅助：实体引用以独立的 `GeneralRef` 事件出现，这里统一还原。
//! 只认 XML 预定义实体与字符引用；其他（DTD 声明的）实体不展开，按原文保留。
use quick_xml::events::BytesRef;

pub fn entity(reference: &BytesRef<'_>) -> String {
    if let Ok(Some(c)) = reference.resolve_char_ref() {
        return c.to_string();
    }
    match &**reference {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        other => format!("&{other};"),
    }
}
