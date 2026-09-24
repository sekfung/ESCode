//! 差分：Rust bash_parse::analyze 与 TS analyzeBashCommand 的解析层 oracle。
//! TS 判为安全（无错误/不支持/动态）的输入必须逐字段一致；其余输入 Rust 只需同样判为不安全。
use serde_json::Value;
use zcode_cli_domain::bash_parse::analyze;

#[test]
fn rust_parser_matches_ts_analysis() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/bash_readonly_corpus.json")).unwrap();
    let mut failures = vec![];
    for case in fixture["parseOracle"].as_array().unwrap() {
        let command = case[0].as_str().unwrap();
        let flags = case[1].as_str().unwrap();
        let a = analyze(command);
        let ts_safe = flags.as_bytes()[..3] == *b"000";
        if !ts_safe {
            if a.permission_safe() {
                failures.push(format!("{command:?}: TS unsafe {flags}, Rust safe"));
            }
            continue;
        }
        let got_flags: String = [
            a.has_parse_errors,
            a.has_unsupported_syntax,
            a.has_dynamic_words,
            a.has_redirects,
        ]
        .iter()
        .map(|b| if *b { '1' } else { '0' })
        .collect();
        let got: Vec<Value> = a
            .commands
            .iter()
            .map(|c| {
                serde_json::json!([
                    c.argv,
                    c.operator_before.unwrap_or(""),
                    c.command_text,
                    c.env
                        .iter()
                        .map(|(n, v)| format!("{n}={v}"))
                        .collect::<Vec<_>>(),
                    c.redirects
                        .iter()
                        .map(|r| format!(
                            "{}{}{}",
                            r.fd.map(|f| f.to_string()).unwrap_or_default(),
                            r.op,
                            r.target
                        ))
                        .collect::<Vec<_>>(),
                ])
            })
            .collect();
        if got_flags != flags || got != case[2].as_array().unwrap().clone() {
            failures.push(format!(
                "{command:?}: TS {flags} {} / Rust {got_flags} {}",
                case[2],
                Value::from(got)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
