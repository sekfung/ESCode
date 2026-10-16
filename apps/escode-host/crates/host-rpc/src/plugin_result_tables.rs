//! `plugins/list`、`plugins/overview` 结果 schema 的声明表（TS `escodePlugin*Schema`，字段顺序与 TS 声明一致）。
//! DSL 与校验器在 `plugin_result_schema.rs`（docs/specs/rust-u2-plugin-management.md §7）。

use serde_json::Value;

use crate::plugin_result_schema::{Ty, opt, req};

static NON_NEG_INT: Ty = Ty::NonNegInt;
static STR: Ty = Ty::Str;
static NON_EMPTY: Ty = Ty::NonEmpty;
static BOOL: Ty = Ty::Bool;
static NUM: Ty = Ty::Num;
static NON_EMPTY_ARRAY: Ty = Ty::Arr(&NON_EMPTY);

static STR_ARRAY: Ty = Ty::Arr(&STR);
static STR_REC: Ty = Ty::Rec(&STR);
static STR_ARRAY_REC: Ty = Ty::Rec(&STR_ARRAY);

static SCOPE: Ty = Ty::Enum(&["user", "workspace"]);
static OPTION_VALUE: Ty = Ty::Union(&[&STR, &NUM, &BOOL]);
static TRUE_OR_STR: Ty = Ty::Union(&[&Ty::True, &STR]);
static POS_NUM: Ty = Ty::PosNum;
static POS_INT: Ty = Ty::PosInt;
static ANY_OBJ: Ty = Ty::AnyObj;
static MISSING: Ty = Ty::Lit("missing");

static HOOK_DETAIL: Ty = Ty::Obj(&[
    req("event", &NON_EMPTY),
    opt("matcher", &STR),
    req("type", &Ty::Enum(&["command", "process"])),
    req("command", &NON_EMPTY),
    opt("args", &STR_ARRAY),
    opt("async", &BOOL),
    opt("shell", &TRUE_OR_STR),
    opt("timeout", &POS_NUM),
    opt("timeoutMs", &POS_INT),
    opt("statusMessage", &STR),
    req("sourcePath", &STR),
    req("runnable", &BOOL),
]);
static HOOK_DETAILS: Ty = Ty::Arr(&HOOK_DETAIL);

static USER_CONFIG_OPTION: Ty = Ty::Obj(&[
    opt("default", &OPTION_VALUE),
    opt("description", &STR),
    opt("required", &BOOL),
    opt("sensitive", &BOOL),
    opt("title", &STR),
    opt(
        "type",
        &Ty::Enum(&["string", "number", "boolean", "directory", "file"]),
    ),
]);

static COMPONENT_ITEM: Ty = Ty::Obj(&[req("name", &NON_EMPTY), opt("description", &STR)]);
static COMPONENT_GROUP: Ty = Ty::Obj(&[
    req(
        "kind",
        &Ty::Enum(&["agent", "command", "skill", "hook", "mcp"]),
    ),
    req("items", &Ty::Arr(&COMPONENT_ITEM)),
]);

static PLUGIN_INFO: Ty = Ty::Obj(&[
    req("id", &NON_EMPTY),
    req("name", &NON_EMPTY),
    opt("description", &STR),
    opt("version", &STR),
    req("enabled", &BOOL),
    req("source", &NON_EMPTY),
    req("marketplace", &NON_EMPTY),
    opt("author", &STR),
    opt("authorUrl", &STR),
    opt("homepage", &STR),
    opt("skillCount", &NON_NEG_INT),
    req("skillRootCount", &NON_NEG_INT),
    req("commandRootCount", &NON_NEG_INT),
    opt("components", &Ty::Arr(&COMPONENT_GROUP)),
    opt("declaredMcpServerNames", &STR_ARRAY),
    opt("hostMcpServerNames", &STR_ARRAY),
    req("mcpServerNames", &STR_ARRAY),
    opt("hookDetails", &HOOK_DETAILS),
    req("rootPath", &STR),
    opt("userConfig", &Ty::Rec(&USER_CONFIG_OPTION)),
    opt("configuredOptions", &Ty::Rec(&OPTION_VALUE)),
    opt("packageStatus", &MISSING),
    opt("rootSource", &SCOPE),
    opt("enabledSource", &SCOPE),
    opt("optionSources", &Ty::Rec(&SCOPE)),
]);

static DIAGNOSTIC: Ty = Ty::Obj(&[
    req("code", &STR),
    req("message", &STR),
    opt("severity", &Ty::Enum(&["warning", "error"])),
    opt("pluginId", &STR),
]);
static DIAGNOSTICS: Ty = Ty::Arr(&DIAGNOSTIC);

/// `escodePluginsListResultSchema`
pub(crate) static PLUGINS_LIST_RESULT: Ty = Ty::Obj(&[
    req("plugins", &Ty::Arr(&PLUGIN_INFO)),
    req("diagnostics", &DIAGNOSTICS),
]);

static STORE_LISTING: Ty = Ty::Obj(&[
    opt("displayName", &STR),
    opt("displayNameI18n", &STR_REC),
    opt("descriptionI18n", &STR_REC),
    opt("icon", &STR),
    opt("category", &STR),
    opt("author", &STR),
    opt("authorUrl", &STR),
    opt("homepage", &STR),
    opt("privacyPolicy", &STR),
    opt("termsOfService", &STR),
    opt("heroImage", &STR),
    opt("examplePrompts", &STR_ARRAY),
    opt("examplePromptsI18n", &STR_ARRAY_REC),
    opt("requiresPaidPlan", &BOOL),
]);

static MARKETPLACE_SUMMARY: Ty = Ty::Obj(&[
    req("id", &NON_EMPTY),
    req("name", &NON_EMPTY),
    req("source", &ANY_OBJ),
    opt("description", &STR),
    opt("lastUpdated", &STR),
    req("pluginCount", &NON_NEG_INT),
    opt("isOfficial", &BOOL),
    opt("featured", &STR_ARRAY),
    opt(
        "refreshFailure",
        &Ty::Obj(&[
            req("code", &STR),
            req("failedAt", &STR),
            req("message", &STR),
        ]),
    ),
]);

static AVAILABLE_PLUGIN: Ty = Ty::Obj(&[
    req("id", &NON_EMPTY),
    req("name", &NON_EMPTY),
    req("marketplace", &NON_EMPTY),
    opt("description", &STR),
    opt("version", &STR),
    req("installed", &BOOL),
    opt("componentTypes", &STR_ARRAY),
    opt("listing", &STORE_LISTING),
]);

static INSTALLED_PLUGIN: Ty = Ty::Obj(&[
    req("id", &NON_EMPTY),
    req("name", &NON_EMPTY),
    req("marketplace", &NON_EMPTY),
    opt("description", &STR),
    opt("version", &STR),
    req("enabled", &BOOL),
    req("scope", &SCOPE),
    opt("installPath", &STR),
    opt("installedAt", &STR),
    opt("componentTypes", &STR_ARRAY),
    opt("hookDetails", &HOOK_DETAILS),
    opt(
        "updateStatus",
        &Ty::Enum(&["none", "update-available", "version-changed"]),
    ),
    opt("latestVersion", &STR),
    opt("listing", &STORE_LISTING),
]);

/// `escodePluginsOverviewResultSchema`
pub(crate) static PLUGINS_OVERVIEW_RESULT: Ty = Ty::Obj(&[
    req("marketplaces", &Ty::Arr(&MARKETPLACE_SUMMARY)),
    req("availablePlugins", &Ty::Arr(&AVAILABLE_PLUGIN)),
    req("installedPlugins", &Ty::Arr(&INSTALLED_PLUGIN)),
    req("restorableBuiltins", &Ty::Arr(&AVAILABLE_PLUGIN)),
    req("diagnostics", &DIAGNOSTICS),
    req(
        "capability",
        &Ty::Obj(&[req("supported", &BOOL), opt("reason", &STR)]),
    ),
]);

/// `escodePluginsMarketplaceMutationResultSchema`
static MARKETPLACE_MUTATION_RESULT: Ty = Ty::Obj(&[
    opt("marketplace", &MARKETPLACE_SUMMARY),
    opt("marketplaces", &Ty::Arr(&MARKETPLACE_SUMMARY)),
    opt("diagnostics", &DIAGNOSTICS),
]);

/// `escodePluginsInstallResultSchema`（install 与 update 共用）
static INSTALL_RESULT: Ty = Ty::Obj(&[
    req("installedPlugins", &Ty::Arr(&INSTALLED_PLUGIN)),
    req("dependencyClosure", &STR_ARRAY),
    req("diagnostics", &DIAGNOSTICS),
]);

static CANCEL_OPERATION_RESULT: Ty =
    Ty::Obj(&[req("operationId", &NON_EMPTY), req("cancelled", &BOOL)]);

static UNINSTALL_RESULT: Ty = Ty::Obj(&[
    opt("removedPlugin", &INSTALLED_PLUGIN),
    req("diagnostics", &DIAGNOSTICS),
]);

/// `escodePluginsRestoreBuiltinResultSchema` 与 `escodePluginsConfigureResultSchema` 同形（configure 与 resetConfig 共用后者）。
static PLUGIN_ID_RESULT: Ty = Ty::Obj(&[
    req("pluginId", &NON_EMPTY),
    req("diagnostics", &DIAGNOSTICS),
]);

static VALIDATE_RESULT: Ty = Ty::Obj(&[
    req("ok", &BOOL),
    req("diagnostics", &DIAGNOSTICS),
    req(
        "compatibility",
        &Ty::Obj(&[
            req("runnable", &STR_ARRAY),
            req("diagnosticOnly", &STR_ARRAY),
            req("unsupported", &STR_ARRAY),
        ]),
    ),
]);

static DESCRIBE_RESULT: Ty = Ty::Obj(&[
    req("components", &Ty::Arr(&COMPONENT_GROUP)),
    opt("diagnostics", &DIAGNOSTICS),
    opt(
        "metadata",
        &Ty::Obj(&[
            opt("author", &STR),
            opt("authorUrl", &STR),
            opt("homepage", &STR),
            opt("version", &STR),
        ]),
    ),
]);

static SET_ENABLED_RESULT: Ty = Ty::Obj(&[req("plugin", &PLUGIN_INFO), req("enabled", &BOOL)]);

/// TS `.superRefine`：可执行的建议结果（ready/disabled/missing）必须带可信的安装身份。
fn require_trusted_identity(value: &Value) -> Result<(), &'static str> {
    let actionable = matches!(
        value["status"].as_str(),
        Some("ready" | "disabled" | "missing")
    );
    let trusted = value.get("marketplace").is_some_and(truthy_string)
        && value.get("pluginName").is_some_and(truthy_string)
        && value["sourceTrust"] == "official";
    if actionable && !trusted {
        return Err("actionable suggested Plugin results require trusted install identity");
    }
    Ok(())
}

/// JS 真值：`marketplace`/`pluginName` 已被 `nonEmptyString` 约束，这里只剩“存在”。
fn truthy_string(value: &Value) -> bool {
    value.as_str().is_some_and(|text| !text.is_empty())
}

/// `escodePluginsResolveSuggestedReferenceResultSchema`
static RESOLVE_SUGGESTED_RESULT: Ty = Ty::Check(
    &Ty::Obj(&[
        req("stableId", &NON_EMPTY),
        req(
            "status",
            &Ty::Enum(&["ready", "disabled", "missing", "conflict", "unavailable"]),
        ),
        opt("marketplace", &NON_EMPTY),
        opt("pluginName", &NON_EMPTY),
        opt("sourceTrust", &Ty::Lit("official")),
        opt("icon", &STR),
        opt("listing", &STORE_LISTING),
        req("diagnostics", &DIAGNOSTICS),
    ]),
    require_trusted_identity,
);

/// `escodePluginReferenceCatalogEntrySchema`：`subagentNames` 是 `.default([])`——缺省时 TS 的 `parse` 会补 `[]`，
/// 所以这里按可选接受，补默认值由 [`normalize`] 做。
static REFERENCE_CATALOG_ENTRY: Ty = Ty::Obj(&[
    opt("category", &NON_EMPTY),
    req("pluginId", &NON_EMPTY),
    req("name", &NON_EMPTY),
    req("marketplace", &NON_EMPTY),
    opt("icon", &STR),
    opt("displayName", &STR),
    opt("displayNameI18n", &STR_REC),
    opt("description", &STR),
    opt("descriptionI18n", &STR_REC),
    req("enabled", &BOOL),
    req("conflictingPluginIds", &NON_EMPTY_ARRAY),
    req("skillQualifiedNames", &NON_EMPTY_ARRAY),
    req("mcpServerNames", &NON_EMPTY_ARRAY),
    opt("subagentNames", &NON_EMPTY_ARRAY),
]);

/// `escodePluginsReferenceCatalogResultSchema`
static REFERENCE_CATALOG_RESULT: Ty = Ty::Obj(&[
    req("authority", &Ty::Enum(&["session", "workspace"])),
    req("plugins", &Ty::Arr(&REFERENCE_CATALOG_ENTRY)),
]);

/// 方法 → 结果 schema。其余方法的 schema 都没有 `default/transform`，`parse` 的输出等于输入
/// （生成器逐条断言这一点）；唯一的例外是引用目录，见 [`normalize`]。
pub(crate) fn schema_for(runtime_method: &str) -> Option<&'static Ty> {
    Some(match runtime_method {
        "plugins/list" => &PLUGINS_LIST_RESULT,
        "plugins/overview" => &PLUGINS_OVERVIEW_RESULT,
        "plugins/marketplace/add" | "plugins/marketplace/remove" | "plugins/marketplace/update" => {
            &MARKETPLACE_MUTATION_RESULT
        }
        "plugins/install" | "plugins/update" => &INSTALL_RESULT,
        "plugins/cancelOperation" => &CANCEL_OPERATION_RESULT,
        "plugins/uninstall" => &UNINSTALL_RESULT,
        "plugins/restoreBuiltin" | "plugins/configure" | "plugins/resetConfig" => &PLUGIN_ID_RESULT,
        "plugins/validate" => &VALIDATE_RESULT,
        "plugins/describe" => &DESCRIBE_RESULT,
        "plugins/setEnabled" => &SET_ENABLED_RESULT,
        "plugins/referenceCatalogWithCategory" | "plugins/referenceCatalog" => {
            &REFERENCE_CATALOG_RESULT
        }
        "plugins/resolveSuggestedReference" => &RESOLVE_SUGGESTED_RESULT,
        _ => return None,
    })
}

/// 校验通过后按 zod 的 `default` 补缺省值（唯一一处：引用目录条目缺 `subagentNames` → `[]`，
/// 旧 Host 不投影该字段时按空数组兼容）。其余方法是恒等变换。
pub(crate) fn normalize(runtime_method: &str, result: &mut Value) {
    if !matches!(
        runtime_method,
        "plugins/referenceCatalogWithCategory" | "plugins/referenceCatalog"
    ) {
        return;
    }
    let Some(entries) = result.get_mut("plugins").and_then(Value::as_array_mut) else {
        return;
    };
    for entry in entries {
        if let Some(object) = entry.as_object_mut() {
            object
                .entry("subagentNames")
                .or_insert_with(|| Value::Array(Vec::new()));
        }
    }
}
