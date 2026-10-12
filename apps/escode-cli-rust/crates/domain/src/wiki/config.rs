//! `config.json` 的 `wiki` 段（spec §12）。只取类型正确的字段，其余回落默认值；
//! 非法值不让整个配置失效（与 MCP 单项 config_invalid 的容错一致），由 `issues` 报告。
use serde_json::Value;

pub const FORMATS: [&str; 13] = [
    "docx", "doc", "xlsx", "xls", "pdf", "markdown", "txt", "csv", "json", "yaml", "xml", "pptx",
    "rst",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTools {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Retrieval {
    pub max_results: usize,
    pub max_injected_tokens: usize,
    pub max_entry_tokens: usize,
    pub min_score: f64,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub max_file_bytes: u64,
    pub max_zip_entries: usize,
    pub max_uncompressed_bytes: u64,
    pub max_compression_ratio: u64,
    pub max_pdf_pages: usize,
    pub max_sheet_cells: usize,
    pub max_records: usize,
    pub max_nesting_depth: usize,
    pub max_nodes: usize,
    pub max_parse_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WikiConfig {
    pub enabled: bool,
    pub root: Option<String>,
    pub auto_inject: bool,
    pub agent_tools: AgentTools,
    pub retrieval: Retrieval,
    pub formats: Vec<(String, bool)>,
    pub limits: Limits,
    pub ocr_enabled: bool,
    pub office_converter: Option<(String, u64)>,
    pub debug: bool,
    pub issues: Vec<String>,
}

impl Default for WikiConfig {
    fn default() -> Self {
        Self::from_value(&Value::Null)
    }
}

impl WikiConfig {
    pub fn format_enabled(&self, format: &str) -> bool {
        self.formats
            .iter()
            .find(|(f, _)| f == format)
            .is_none_or(|(_, on)| *on)
    }

    /// 配置指纹：进入检索缓存键，配置变化后旧缓存不再命中。
    pub fn fingerprint(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.retrieval.max_results,
            self.retrieval.max_injected_tokens,
            self.retrieval.max_entry_tokens,
            self.retrieval.min_score,
            self.auto_inject
        )
    }

    pub fn from_value(wiki: &Value) -> Self {
        let mut issues = vec![];
        let mut boolean = |key: &str, default: bool, value: &Value| -> bool {
            match value.get(key) {
                None | Some(Value::Null) => default,
                Some(Value::Bool(b)) => *b,
                Some(_) => {
                    issues.push(format!("wiki.{key} must be a boolean"));
                    default
                }
            }
        };
        let enabled = boolean("enabled", true, wiki);
        let auto_inject = boolean("autoInject", true, wiki);
        let debug = boolean("debug", false, wiki);
        let ocr_enabled = wiki
            .get("ocr")
            .is_some_and(|o| boolean("enabled", false, o));
        let office = wiki
            .pointer("/converters/office")
            .cloned()
            .unwrap_or(Value::Null);
        let office_on = boolean("enabled", false, &office);
        let num = |pointer: &str, default: u64, issues: &mut Vec<String>| -> u64 {
            match wiki.pointer(pointer) {
                None | Some(Value::Null) => default,
                Some(v) => v.as_u64().filter(|n| *n > 0).unwrap_or_else(|| {
                    issues.push(format!(
                        "wiki{} must be a positive integer",
                        pointer.replace('/', ".")
                    ));
                    default
                }),
            }
        };
        let agent_tools = match wiki.get("agentTools").and_then(Value::as_str) {
            None => AgentTools::Auto,
            Some("auto") => AgentTools::Auto,
            Some("always") => AgentTools::Always,
            Some("never") => AgentTools::Never,
            Some(other) => {
                issues.push(format!(
                    "wiki.agentTools '{other}' is not auto|always|never"
                ));
                AgentTools::Auto
            }
        };
        let min_score = match wiki.pointer("/retrieval/minScore") {
            None | Some(Value::Null) => 0.3,
            Some(v) => v
                .as_f64()
                .filter(|s| (0.0..=1.0).contains(s))
                .unwrap_or_else(|| {
                    issues.push("wiki.retrieval.minScore must be within 0..1".into());
                    0.3
                }),
        };
        const MIB: u64 = 1024 * 1024;
        let retrieval = Retrieval {
            max_results: num("/retrieval/maxResults", 5, &mut issues).min(20) as usize,
            max_injected_tokens: num("/retrieval/maxInjectedTokens", 1_500, &mut issues) as usize,
            max_entry_tokens: num("/retrieval/maxEntryTokens", 600, &mut issues) as usize,
            min_score,
            timeout_ms: num("/retrieval/timeoutMs", 3_000, &mut issues),
        };
        let limits = Limits {
            max_file_bytes: num("/limits/maxFileBytes", 25 * MIB, &mut issues),
            max_zip_entries: num("/limits/maxZipEntries", 5_000, &mut issues) as usize,
            max_uncompressed_bytes: num("/limits/maxUncompressedBytes", 200 * MIB, &mut issues),
            max_compression_ratio: num("/limits/maxCompressionRatio", 200, &mut issues),
            max_pdf_pages: num("/limits/maxPdfPages", 500, &mut issues) as usize,
            max_sheet_cells: num("/limits/maxSheetCells", 200_000, &mut issues) as usize,
            max_records: num("/limits/maxRecords", 100_000, &mut issues) as usize,
            max_nesting_depth: num("/limits/maxNestingDepth", 64, &mut issues) as usize,
            max_nodes: num("/limits/maxNodes", 200_000, &mut issues) as usize,
            max_parse_ms: num("/limits/maxParseMs", 30_000, &mut issues),
        };
        let formats = FORMATS
            .iter()
            .map(|f| {
                let on = wiki
                    .pointer(&format!("/formats/{f}"))
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                ((*f).to_owned(), on)
            })
            .collect();
        let command = office
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("soffice")
            .to_owned();
        let office_timeout = num("/converters/office/timeoutMs", 120_000, &mut issues);
        if wiki
            .get("projectStorage")
            .and_then(Value::as_str)
            .is_some_and(|m| m != "user-data")
        {
            issues.push("wiki.projectStorage only supports \"user-data\"".into());
        }
        WikiConfig {
            enabled,
            root: wiki
                .get("root")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            auto_inject,
            agent_tools,
            retrieval,
            formats,
            limits,
            ocr_enabled,
            office_converter: office_on.then_some((command, office_timeout)),
            debug,
            issues,
        }
    }
}
