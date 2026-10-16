//! `.escodeignore` 规则（TS `workspaceFileIgnore.ts`）：加载 / 模板 / 分区转换 / 原子写。
//! 规则语法是 gitignore；匹配委托 `ignore` crate（TS 委托 `ignore` npm 包），禁止手写解析。

use escode_host_control::logging;
use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use tokio::io::AsyncWriteExt;

pub const IGNORE_FILE_NAME: &str = ".escodeignore";
const GITIGNORE_FILE_NAME: &str = ".gitignore";

const BUILTIN_IGNORE_LINES: [&str; 28] = [
    ".git/",
    ".hg/",
    ".svn/",
    "node_modules/",
    "bower_components/",
    "jspm_packages/",
    "__pycache__/",
    "site-packages/",
    "venv/",
    "coverage/",
    "htmlcov/",
    "lcov-report/",
    "cmakefiles/",
    "cmake-build-*/",
    "bazel-*/",
    "pods/",
    "deriveddata/",
    "storybook-static/",
    "playwright-report/",
    "test-results/",
    "allure-results/",
    "allure-report/",
    "cdk.out/",
    "*.egg-info/",
    "*.dist-info/",
    "eggs/",
    "pip-wheel-metadata/",
    "wheels/",
];

const TEMPLATE_HEADER: [&str; 5] = [
    "# ESCode 工作区文件搜索忽略规则（.escodeignore）",
    "# 语法与 .gitignore 一致，只影响 ESCode 的 @ 文件候选 / Command Center / 文件树搜索，",
    "# 不影响文件树浏览、上传或 Agent 文件访问。",
    "# 修改 .gitignore 不会自动同步到本文件；可在设置页「从 .gitignore 同步」。",
    "",
];

pub const SYNC_MARKER: &str =
    "# ===== ↑ 以上同步自 .gitignore（「从 .gitignore 同步」只重写以上部分）=====";
pub const DEFAULTS_MARKER: &str =
    "# ----- ↑ 以上为 ESCode 默认排除规则（自定义规则请写在本行下方，不会被同步/恢复改动）-----";
const CUSTOM_SECTION_HINT: &str = "# 自定义规则写在下方（本行提示可删除）";

/// JS `/\r?\n/` 拆行。
fn split_lines(content: &str) -> Vec<&str> {
    content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn js_trim(value: &str) -> &str {
    crate::git::paths::js_trim(value)
}

/// TS `buildBuiltinDefaultsSection`：gitignore 区已声明的规则不重复写入默认段。
fn builtin_defaults_section(gitignore: Option<&str>) -> String {
    let Some(content) = gitignore else {
        return BUILTIN_IGNORE_LINES.join("\n");
    };
    let mut declared: Vec<String> = Vec::new();
    for raw in split_lines(content) {
        let line = js_trim(raw);
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        declared.push(line.to_owned());
        declared.push(line.strip_suffix('/').unwrap_or(line).to_owned());
    }
    BUILTIN_IGNORE_LINES
        .iter()
        .filter(|line| {
            let bare = line.strip_suffix('/').unwrap_or(line);
            !declared.iter().any(|d| d == *line || d == bare)
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

fn gitignore_section(gitignore: Option<&str>) -> String {
    match gitignore {
        Some(content) if !js_trim(content).is_empty() => {
            if content.ends_with('\n') {
                content.to_owned()
            } else {
                format!("{content}\n")
            }
        }
        _ => format!("{}\n", TEMPLATE_HEADER.join("\n")),
    }
}

/// TS `buildWorkspaceFileSearchIgnoreTemplate`。
pub fn build_template(gitignore: Option<&str>) -> String {
    [
        gitignore_section(gitignore),
        SYNC_MARKER.to_owned(),
        builtin_defaults_section(gitignore),
        DEFAULTS_MARKER.to_owned(),
        CUSTOM_SECTION_HINT.to_owned(),
        String::new(),
    ]
    .join("\n")
}

struct Sections {
    gitignore: String,
    defaults: String,
    custom: String,
}

/// 按双标记切分；任一标记缺失返回 `None`。
fn split_sections(content: &str) -> Option<Sections> {
    let lines = split_lines(content);
    let sync = lines.iter().position(|l| js_trim(l) == SYNC_MARKER)?;
    let defaults = lines.iter().position(|l| js_trim(l) == DEFAULTS_MARKER)?;
    if defaults <= sync {
        return None;
    }
    Some(Sections {
        gitignore: lines[..sync].join("\n"),
        defaults: js_trim(&lines[sync + 1..defaults].join("\n")).to_owned(),
        custom: lines[defaults + 1..]
            .join("\n")
            .trim_start_matches('\n')
            .to_owned(),
    })
}

fn recompose(parts: [String; 5]) -> String {
    // TS `.replace(/\n+$/, "\n")`：只在以换行结尾时归一为单个换行，否则原样。
    let joined = parts.join("\n");
    if joined.ends_with('\n') {
        format!("{}\n", joined.trim_end_matches('\n'))
    } else {
        joined
    }
}

/// TS `syncWorkspaceFileSearchIgnoreFromGitignore`。
pub fn sync_from_gitignore(current: &str, gitignore: Option<&str>) -> String {
    let Some(sections) = split_sections(current) else {
        return build_template(gitignore);
    };
    recompose([
        gitignore_section(gitignore),
        SYNC_MARKER.to_owned(),
        sections.defaults,
        DEFAULTS_MARKER.to_owned(),
        sections.custom,
    ])
}

/// TS `resetWorkspaceFileSearchIgnoreDefaults`。
pub fn reset_defaults(current: &str, gitignore: Option<&str>) -> String {
    let Some(sections) = split_sections(current) else {
        return build_template(gitignore);
    };
    let defaults = builtin_defaults_section(Some(&sections.gitignore));
    recompose([
        sections.gitignore,
        SYNC_MARKER.to_owned(),
        defaults,
        DEFAULTS_MARKER.to_owned(),
        sections.custom,
    ])
}

/// 已加载的规则。
pub struct Rules {
    matcher: Gitignore,
}

impl Rules {
    /// TS `buildIgnoreMatcher`：整段内容按行加入。
    pub fn from_content(content: &str) -> Self {
        let mut builder = GitignoreBuilder::new("");
        // npm `ignore` 默认大小写不敏感（ignorecase: true）：`readme.md` 会忽略 `README.md`。
        let _ = builder.case_insensitive(true);
        for line in split_lines(content) {
            // 单行语法错误（如坏字符类）在 npm `ignore` 里按字面/宽容处理；这里跳过坏行，其余规则照常生效。
            let _ = builder.add_line(None, line);
        }
        Self {
            matcher: builder.build().unwrap_or_else(|_| Gitignore::empty()),
        }
    }

    /// TS `isWorkspaceFileSearchPathIgnored`：`relative` 为 posix 相对路径；父目录被排除则整体排除。
    pub fn ignores(&self, relative: &str, directory: bool) -> bool {
        self.matcher
            .matched_path_or_any_parents(relative, directory)
            .is_ignore()
    }
}

async fn read_optional(path: &Path) -> std::io::Result<Option<String>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// TS `loadWorkspaceFileSearchIgnoreRules`：文件存在用其内容；不存在在内存里用模板（不落盘）；
/// 读取失败降级 `.gitignore` → 内置默认，只 warn。
pub async fn load_rules(root: &str) -> Rules {
    let root = Path::new(root);
    let degraded = |reason: &str, gitignore: Option<String>| {
        logging::warn(
            "file-index",
            format!("[workspace-file-ignore] {reason}，降级为运行时使用规则"),
        );
        Rules::from_content(&match gitignore {
            Some(content) => content,
            None => build_template(None),
        })
    };
    match read_optional(&root.join(IGNORE_FILE_NAME)).await {
        Ok(Some(content)) => Rules::from_content(&content),
        Ok(None) => {
            let gitignore = read_optional(&root.join(GITIGNORE_FILE_NAME))
                .await
                .ok()
                .flatten();
            Rules::from_content(&build_template(gitignore.as_deref()))
        }
        Err(_) => {
            let gitignore = read_optional(&root.join(GITIGNORE_FILE_NAME))
                .await
                .ok()
                .flatten();
            degraded(&format!("读取 {IGNORE_FILE_NAME} 失败"), gitignore)
        }
    }
}

/// 设置页读取：`(content, "file"|"template")`。
pub async fn read_for_settings(root: &str) -> std::io::Result<(String, &'static str)> {
    let root = Path::new(root);
    if let Some(existing) = read_optional(&root.join(IGNORE_FILE_NAME)).await? {
        return Ok((existing, "file"));
    }
    let gitignore = read_optional(&root.join(GITIGNORE_FILE_NAME))
        .await
        .ok()
        .flatten();
    Ok((build_template(gitignore.as_deref()), "template"))
}

/// 设置页分区操作。
pub async fn transform(root: &str, kind: &str) -> anyhow::Result<String> {
    let root = Path::new(root);
    let existing = read_optional(&root.join(IGNORE_FILE_NAME))
        .await
        .ok()
        .flatten();
    let gitignore = read_optional(&root.join(GITIGNORE_FILE_NAME))
        .await
        .ok()
        .flatten();
    let current = existing.unwrap_or_else(|| build_template(gitignore.as_deref()));
    Ok(match kind {
        "sync-gitignore" => sync_from_gitignore(&current, gitignore.as_deref()),
        _ => reset_defaults(&current, gitignore.as_deref()),
    })
}

/// 原子写：同目录临时文件（flush + fsync）后 rename 替换。
pub async fn write_atomic(root: &str, content: &str) -> std::io::Result<()> {
    let path = Path::new(root).join(IGNORE_FILE_NAME);
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let temp = directory.join(format!(
        ".{IGNORE_FILE_NAME}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .await?;
        file.write_all(content.as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temp, &path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    result
}
