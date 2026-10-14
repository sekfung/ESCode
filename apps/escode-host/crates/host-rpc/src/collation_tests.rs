use super::*;

/// 环境表解析（生产走 `default_locale` + `core_api::env`；测试注入固定表）。
fn resolve(table: &[(&str, &str)]) -> String {
    default_locale(|name| {
        table
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    })
}

#[test]
fn locale_resolution_follows_node_precedence() {
    // Node 24 实测（`new Intl.Collator().resolvedOptions().locale`）：见下表逐项。
    let cases: [(Vec<(&str, &str)>, &str); 8] = [
        (vec![], "en-US"),
        (vec![("LANG", "zh_CN.UTF-8")], "zh-CN"),
        (vec![("LANG", "de_DE.UTF-8")], "de-DE"),
        (vec![("LANG", "C")], "en-US"),
        (vec![("LANG", "POSIX")], "en-US"),
        (vec![("LC_ALL", "C.UTF-8")], "en-US"),
        (vec![("LANG", "xx_YY.UTF-8")], "xx-YY"),
        (vec![("LANG", "zh_Hans_CN.UTF-8")], "zh-Hans-CN"),
    ];
    for (pairs, expected) in cases {
        assert_eq!(resolve(&pairs), expected, "{pairs:?}");
    }
}

#[test]
fn precedence_prefers_the_first_set_variable() {
    assert_eq!(
        resolve(&[
            ("LANG", "zh_CN.UTF-8"),
            ("LC_MESSAGES", "fr_FR.UTF-8"),
        ]),
        "fr-FR",
        "LC_MESSAGES 覆盖 LANG"
    );
    assert_eq!(
        resolve(&[("LANG", "zh_CN.UTF-8"), ("LC_ALL", "en_GB.UTF-8")]),
        "en-GB",
        "LC_ALL 覆盖其余"
    );
    // 空值不生效（Node `LC_ALL= node` 仍是 LANG）。
    assert_eq!(
        resolve(&[("LANG", "de_DE.UTF-8"), ("LC_ALL", "  ")]),
        "de-DE"
    );
}

#[test]
fn ordering_matches_node_locale_compare_for_root_and_zh() {
    // 语料：文件名字符集（大写/小写/重音/连字符/点/括号/波浪号/数字/中日文）。
    let names = [
        "_x",
        "a",
        "A",
        "dir_b",
        "dirA",
        "é.txt",
        "README.md",
        "z.txt",
        "Z",
        "1.txt",
        "10.txt",
        "2.txt",
        "a1",
        "a2",
        "a10",
        "a 1",
        "a_1",
        "a-1",
        "a.1",
        "file.TXT",
        "FILE.txt",
        "file(1).txt",
        "file[1].txt",
        "file~1.txt",
        "ö",
        "Ø",
        "ß",
        "Ω",
    ];
    // en-US 的期望顺序：由 `node -e 'names.sort((a,b)=>a.localeCompare(b))'` 生成（Node 24.14）。
    let expected_en = [
        "_x", "1.txt", "10.txt", "2.txt", "a", "A", "a 1", "a_1", "a-1", "a.1", "a1", "a10", "a2",
        "dir_b", "dirA", "é.txt", "file.TXT", "FILE.txt", "file(1).txt", "file[1].txt",
        "file~1.txt", "ö", "Ø", "README.md", "ß", "Z", "z.txt", "Ω",
    ];
    let mut sorted: Vec<&str> = names.to_vec();
    let collator = build("en-US");
    sorted.sort_by(|left, right| collator.compare(left, right));
    assert_eq!(sorted, expected_en.to_vec(), "en-US 顺序与 Node 一致");
}
