//! 差分：Rust builtin_prompt_command 与 TS resolveZCodeBuiltinPromptCommand。
//! 语料里的路径分隔符随生成平台（Windows `\`、POSIX `/`），比较前统一归一。
use serde_json::Value;
use std::path::Path;
use zcode_cli_domain::builtin_prompt_command::resolve_builtin_prompt_command;

fn normalize(text: &str) -> String {
    text.replace('\\', "/")
}

#[test]
fn rust_init_prompt_matches_ts() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/init_prompt_corpus.json")).unwrap();
    let cwd = Path::new(fixture["workingDirectory"].as_str().unwrap());
    let mut failures = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let input = case[0].as_str().unwrap();
        let want = case[1].as_str().map(normalize);
        let got = resolve_builtin_prompt_command(input, cwd).map(|p| normalize(&p));
        if got != want {
            let describe = |v: &Option<String>| {
                v.as_ref().map_or("null".to_owned(), |p| format!("{} chars", p.len()))
            };
            failures.push(format!(
                "{input:?}: rust {} != ts {}",
                describe(&got),
                describe(&want)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches: {failures:?}",
        failures.len()
    );
}
