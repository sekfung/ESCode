//! `svn:externals` 只读读取（TS `svnExternals.ts`）。

use serde_json::{Value, json};

use super::parse::parse_properties;
use super::paths::to_workspace_relative;
use super::{RunRequest, SvnService, text};

pub(super) struct ParsedExternal {
    pub url: String,
    pub name: String,
    pub revision: Option<i64>,
    pub peg_revision: Option<i64>,
}
pub(super) fn parse_external_line(line: &str) -> Option<ParsedExternal> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let mut index = 0usize;
    let mut revision = None;
    if tokens.first() == Some(&"-r") {
        let value = tokens.get(1)?.parse::<i64>().ok()?;
        if value < 0 {
            return None;
        }
        revision = Some(value);
        index = 2;
    }
    let raw_url = *tokens.get(index)?;
    if index + 1 >= tokens.len() {
        return None;
    }
    let mut name = tokens[index + 1..].join(" ");
    if name.len() >= 2 && name.starts_with('"') && name.ends_with('"') {
        name = name[1..name.len() - 1].to_owned();
    }
    if name.is_empty() {
        return None;
    }
    let (url, peg_revision) = match raw_url.rsplit_once('@') {
        Some((url, revision))
            if !revision.is_empty() && revision.chars().all(|ch| ch.is_ascii_digit()) =>
        {
            (url.to_owned(), revision.parse::<i64>().ok())
        }
        _ => (raw_url.to_owned(), None),
    };
    Some(ParsedExternal {
        url,
        name,
        revision,
        peg_revision,
    })
}

pub(crate) async fn get_externals(service: &SvnService, params: &Value) -> Value {
    let workspace_path = text(params, "workspacePath");
    let output = match service
        .run(RunRequest::new(
            &workspace_path,
            vec![
                "propget".to_owned(),
                "--xml".to_owned(),
                "-R".to_owned(),
                "svn:externals".to_owned(),
                "--".to_owned(),
                ".".to_owned(),
            ],
        ))
        .await
    {
        Ok(output) => output,
        Err(error) => return error,
    };
    let mut externals = Vec::new();
    for entry in parse_properties(&output.stdout, "svn:externals") {
        let definition_path = to_workspace_relative(&workspace_path, &entry.path);
        for line in entry.value.lines() {
            let Some(parsed) = parse_external_line(line) else {
                continue;
            };
            let mut value = json!({
                "path": definition_path,
                "name": parsed.name,
                "url": parsed.url,
            });
            if let Some(revision) = parsed.revision {
                value["revision"] = Value::from(revision);
            }
            if let Some(peg) = parsed.peg_revision {
                value["pegRevision"] = Value::from(peg);
            }
            externals.push(value);
        }
    }
    externals.sort_by(|left, right| {
        let path = left["path"].as_str().unwrap_or_default();
        let other_path = right["path"].as_str().unwrap_or_default();
        path.cmp(other_path).then_with(|| {
            left["name"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["name"].as_str().unwrap_or_default())
        })
    });
    json!({ "ok": true, "externals": externals })
}
