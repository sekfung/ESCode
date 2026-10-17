//! frontmatter 解析与生成（`rust-u2-commands.md` §1，TS `commandFileParser.ts`）。
use super::*;

#[test]
fn parse_splits_frontmatter_from_the_body() {
    let content = "---\ndescription: 生成周报\nargument-hint: <范围>\n---\n\n帮我写周报\n";
    let parsed = parse_command_file(content, "/home/me/.escode/commands/weekly.md").unwrap();
    assert_eq!(parsed.name, "/weekly");
    assert_eq!(parsed.description.as_deref(), Some("生成周报"));
    assert_eq!(parsed.argument_hint.as_deref(), Some("<范围>"));
    // body = 第二个 `---` 之后的全部行（首个换行保留 → 前导空行），prompt 是它 trim 后的结果。
    assert_eq!(parsed.content, "\n帮我写周报\n");
    assert_eq!(parsed.prompt, "帮我写周报");
}

/// 没有 frontmatter 时整段都是 body；扩展名去掉是大小写不敏感的。
#[test]
fn parse_without_frontmatter_keeps_the_whole_body() {
    let parsed = parse_command_file("只是一段文本", "/c/Upper.MD").unwrap();
    assert_eq!(parsed.name, "/Upper", "只去扩展名，不改大小写");
    assert_eq!(parsed.content, "只是一段文本");
    assert_eq!(parsed.description, None);
}

/// 多行值只收**缩进续行**，并以空格拼接；遇到下一个键即停止。
#[test]
fn multiline_values_collect_indented_continuations_only() {
    let content =
        "---\ndescription: 第一行\n  第二行\n    第三行\nother: x\n  不该被收进来\n---\nbody";
    let parsed = parse_command_file(content, "/c/a.md").unwrap();
    assert_eq!(parsed.description.as_deref(), Some("第一行 第二行 第三行"));
    assert_eq!(parsed.argument_hint, None, "未声明的键不回填");
}

#[test]
fn generate_emits_frontmatter_only_when_there_is_something_to_emit() {
    let bare = generate_command_file_content(
        &CommandConfig {
            prompt: "只有正文",
            description: None,
            argument_hint: None,
        },
        None,
    );
    assert_eq!(bare, "只有正文", "无 frontmatter 时原样输出 prompt");

    let full = generate_command_file_content(
        &CommandConfig {
            prompt: "正文",
            description: Some("  描述  "),
            argument_hint: Some("<x>"),
        },
        None,
    );
    assert_eq!(
        full,
        "---\ndescription: 描述\nargument-hint: <x>\n---\n\n正文"
    );

    // 全空白等于没给。
    let blank = generate_command_file_content(
        &CommandConfig {
            prompt: "p",
            description: Some("   "),
            argument_hint: Some("\t"),
        },
        None,
    );
    assert_eq!(blank, "p");
}

/// 生成时保留未替换的键；被替换的键连同它的**缩进续行**一起丢掉。
#[test]
fn generate_preserves_other_keys_and_replaces_the_managed_ones() {
    let existing =
        "---\nmodel: opus\ndescription: 旧描述\n  旧续行\nargument-hint: <old>\n---\n\n旧正文";
    let generated = generate_command_file_content(
        &CommandConfig {
            prompt: "新正文",
            description: Some("新描述"),
            argument_hint: None,
        },
        Some(existing),
    );
    assert_eq!(
        generated,
        "---\nmodel: opus\ndescription: 新描述\n---\n\n新正文"
    );
}
