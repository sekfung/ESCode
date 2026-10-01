//! 已保存工作流存储层（docs/specs/rust-dynamic-workflow.md 第 2 期），对齐 TS
//! `core/src/tool/handlers/saved-workflows/store.ts`。
//!
//! 用同步 fs：TS 侧的 `prepareApproval` 契约是同步的，以 `saved` 源发起的 run 必须在确认窗
//! **之前**读出脚本（没有脚本就没有因果图）。这些文件是本地的、单个的、以 KB 计的。
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use zcode_cli_domain::{json_order::Json, saved_workflow as codec};

#[allow(unused_imports)]
pub use super::saved_workflows_hub::{hub_delete, hub_get, hub_list, hub_move, hub_update_meta};
#[allow(unused_imports)]
pub use super::saved_workflows_model::{
    display, entry_json, format_model_content, model_content, output, to_value,
};

#[allow(unused_imports)]
pub use super::saved_workflows_move::{
    Move, describe, file_exists, is_cross_device, is_not_found, js_cmp, move_to_project,
};

/// 保存文件的扩展名（`.dwf.ts`：编辑按 TypeScript 高亮，扫描不必打开文件）。
pub const FILE_EXTENSION: &str = ".dwf.ts";

/// 项目作用域目录（相对会话工作目录）。
pub const PROJECT_DIR: &str = ".zcode/workflows";

/// 全局作用域目录（相对 agent 进程家目录）。
pub const GLOBAL_DIR: &str = ".zcode/workflows";

/// 名字长度上限。
pub const MAX_NAME_CHARS: usize = 64;

pub(crate) const INVALID_NAME_DETAIL: &str = "workflow names may only contain letters, digits, '.', '-' and '_', and must be 1-64 characters";

pub(crate) const UNUSABLE_FILE_NAME: &str = "file name is not a usable workflow name";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Project,
    Global,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "project" => Some(Self::Project),
            "global" => Some(Self::Global),
            _ => None,
        }
    }
}

pub struct Root {
    pub scope: Scope,
    pub dir: PathBuf,
}

/// 本次会话的查找根，**按优先级排列**：`[project, global]`，查找 first-wins（项目档遮蔽全局档）。
pub fn roots(cwd: &Path, home: &Path) -> Vec<Root> {
    vec![
        Root {
            scope: Scope::Project,
            dir: join_dir(cwd, PROJECT_DIR),
        },
        Root {
            scope: Scope::Global,
            dir: join_dir(home, GLOBAL_DIR),
        },
    ]
}

/// TS `path.join(base, dir)`：常量里的 `/` 是路径分隔符，必须按平台归一。`Path::join` 不会把
/// `a/b` 拆成两级，Windows 上会留下 `\.zcode/workflows\` 这种混合分隔符，于是输出/display 里的
/// 路径与 Node 逐字不同（App 差分抓到）。
fn join_dir(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(base.to_owned(), |path, part| path.join(part))
}

pub fn root(cwd: &Path, scope: Scope, home: &Path) -> Root {
    roots(cwd, home)
        .into_iter()
        .find(|candidate| candidate.scope == scope)
        .expect("scope 是枚举成员，roots 覆盖全部成员")
}

/// 名字 → 文件名。名字已经过 [`is_valid_name`]，此处不再兜底。
pub fn file_name(name: &str) -> String {
    format!("{name}{FILE_EXTENSION}")
}

/// 名字在给定根下的落点（写侧与读侧共用，避免"保存成功但读不出来"）。
pub fn saved_path(root: &Root, name: &str) -> PathBuf {
    root.dir.join(file_name(name))
}

/// TS `isValidSavedWorkflowName`：字符集 + 长度 + 排除 `.`/`..`。这条检查**就是**路径穿越的防线。
pub fn is_valid_name(name: &str) -> bool {
    let chars: Vec<char> = name.chars().collect();
    if chars.is_empty() || chars.len() > MAX_NAME_CHARS {
        return false;
    }
    if !chars
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return false;
    }
    chars.iter().any(|c| *c != '.')
}

pub struct Resolved {
    pub name: String,
    pub path: PathBuf,
    pub scope: Scope,
    pub meta: Json,
    pub script: String,
    /// 文件原文（元数据块 + 正文）**逐字节**：草稿拷贝拿的就是它。
    pub source: String,
    pub body_line_offset: usize,
}

pub enum Resolve {
    Found(Box<Resolved>),
    InvalidName {
        detail: String,
    },
    NotFound,
    ParseError {
        path: PathBuf,
        reason: String,
        detail: String,
    },
    ReadError {
        path: PathBuf,
        detail: String,
    },
}

/// 按名字解析一个保存的 workflow。给 `scope` 只查那一根，不给则两根按序 first-wins。
pub fn resolve(cwd: &Path, home: &Path, name: &str, scope: Option<Scope>) -> Resolve {
    if !is_valid_name(name) {
        return Resolve::InvalidName {
            detail: INVALID_NAME_DETAIL.to_owned(),
        };
    }
    let dirs = match scope {
        Some(scope) => vec![root(cwd, scope, home)],
        None => roots(cwd, home),
    };
    for candidate in dirs {
        let path = saved_path(&candidate, name);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            // 这一根没有它，看下一根；其余读错（权限、是目录）是**这个**文件的问题。
            Err(error) if is_not_found(&error) => continue,
            Err(error) => {
                return Resolve::ReadError {
                    path,
                    detail: describe(&error),
                };
            }
        };
        match codec::parse(&source) {
            Ok(parsed) => {
                return Resolve::Found(Box::new(Resolved {
                    name: name.to_owned(),
                    path,
                    scope: candidate.scope,
                    meta: parsed.meta,
                    script: parsed.script,
                    source,
                    body_line_offset: parsed.body_line_offset,
                }));
            }
            Err(failure) => {
                return Resolve::ParseError {
                    path,
                    reason: failure.reason.to_owned(),
                    detail: failure.detail,
                };
            }
        }
    }
    Resolve::NotFound
}

pub struct Entry {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub args: Option<Json>,
    pub scope: Scope,
    pub path: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InvalidKind {
    /// 文件名不是一个可用的 workflow 名字。
    NotAWorkflowName,
    /// 文件存在但读不出来。
    ReadError,
    /// frontmatter 解析或元数据校验失败。
    ParseError,
}

pub struct Invalid {
    pub path: PathBuf,
    pub kind: InvalidKind,
    pub reason: String,
}

pub struct Listed {
    pub entries: Vec<Entry>,
    pub invalid: Vec<Invalid>,
}

/// 枚举保存定义（深度 1 的平铺扫描）。坏文件进 `invalid` 而不是让整份清单消失。
///
/// 不给 `scope`：两根按序，同名 first-wins，被遮蔽的那份**不**列出（列表要说的是"调用这个名字
/// 会跑到什么"）。给了 `scope`：只扫那一根，不做遮蔽。
pub fn list(cwd: &Path, home: &Path, scope: Option<Scope>) -> Listed {
    let mut entries = Vec::new();
    let mut invalid = Vec::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    let dirs = match scope {
        Some(scope) => vec![root(cwd, scope, home)],
        None => roots(cwd, home),
    };
    for candidate in dirs {
        let mut names: Vec<String> = match std::fs::read_dir(&candidate.dir) {
            Ok(children) => children
                .filter_map(Result::ok)
                .map(|child| child.file_name().to_string_lossy().into_owned())
                .collect(),
            // 目录不存在是常态（大多数项目没保存过 workflow），不是错误。
            Err(error) if is_not_found(&error) => continue,
            Err(error) => {
                invalid.push(Invalid {
                    path: candidate.dir.clone(),
                    kind: InvalidKind::ReadError,
                    reason: describe(&error),
                });
                continue;
            }
        };
        // readdir 的顺序随文件系统而定，排序让列表在两台机器上一致（JS 默认排序按 UTF-16 码元）。
        names.sort_by(|a, b| js_cmp(a, b));
        for file in names {
            let Some(name) = file.strip_suffix(FILE_EXTENSION) else {
                continue;
            };
            let path = candidate.dir.join(&file);
            if !is_valid_name(name) {
                invalid.push(Invalid {
                    path,
                    kind: InvalidKind::NotAWorkflowName,
                    reason: UNUSABLE_FILE_NAME.to_owned(),
                });
                continue;
            }
            // 已被更高优先级的作用域认领：这一份跑不到，也就不列。
            if claimed.contains(name) {
                continue;
            }
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) => {
                    invalid.push(Invalid {
                        path,
                        kind: InvalidKind::ReadError,
                        reason: describe(&error),
                    });
                    continue;
                }
            };
            match codec::parse(&source) {
                Err(failure) => invalid.push(Invalid {
                    path,
                    kind: InvalidKind::ParseError,
                    reason: format!("{}: {}", failure.reason, failure.detail),
                }),
                Ok(parsed) => {
                    claimed.insert(name.to_owned());
                    entries.push(Entry {
                        name: name.to_owned(),
                        description: parsed
                            .meta
                            .get("description")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        when_to_use: parsed
                            .meta
                            .get("whenToUse")
                            .and_then(Json::as_str)
                            .map(str::to_owned),
                        args: parsed.meta.get("args").cloned(),
                        scope: candidate.scope,
                        path,
                    });
                }
            }
        }
    }
    Listed { entries, invalid }
}

pub struct Saved {
    pub path: PathBuf,
    pub scope: Scope,
    pub overwritten: bool,
}

/// 写入一个保存定义（缺省 `project`），返回落点与"这次是不是覆盖"。
pub fn save(
    cwd: &Path,
    home: &Path,
    name: &str,
    meta: &Json,
    script: &str,
    scope: Option<Scope>,
) -> std::io::Result<Saved> {
    let root = root(cwd, scope.unwrap_or(Scope::Project), home);
    let path = saved_path(&root, name);
    std::fs::create_dir_all(&root.dir)?;
    let overwritten = file_exists(&path);
    std::fs::write(&path, codec::serialize(meta, script))?;
    Ok(Saved {
        path,
        scope: root.scope,
        overwritten,
    })
}

/// 目标是否已存在（确认窗要把"覆盖"与"新建"说成两件事）。缺省查项目档。
pub fn exists(cwd: &Path, home: &Path, name: &str, scope: Option<Scope>) -> bool {
    if !is_valid_name(name) {
        return false;
    }
    let root = root(cwd, scope.unwrap_or(Scope::Project), home);
    file_exists(&saved_path(&root, name))
}

/// 保存到 `scope` 时，另一档是否已有同名定义（遮蔽事实，供确认窗展示）。
pub fn shadowing(cwd: &Path, home: &Path, name: &str, scope: Scope) -> Option<&'static str> {
    if !is_valid_name(name) {
        return None;
    }
    let other = match scope {
        Scope::Project => Scope::Global,
        Scope::Global => Scope::Project,
    };
    if !file_exists(&saved_path(&root(cwd, other, home), name)) {
        return None;
    }
    Some(match scope {
        Scope::Project => "hides_global",
        Scope::Global => "hidden_by_project",
    })
}
