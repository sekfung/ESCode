use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn err(list: &[&str]) -> ArgError {
    parse(args(list)).unwrap_err()
}

#[test]
fn parses_node_compatible_options() {
    let p = parse(args(&[
        "-p",
        "hi",
        "--output-format",
        "json",
        "--mode",
        "YOLO",
        "--cwd",
        "/w",
        "--attach",
        "a.png",
        "--attach",
        "b.txt",
        "--surface",
        "desktop",
        "--verbose",
        "--locale",
        "zh-CN",
    ]))
    .unwrap();
    assert_eq!(p.prompt.as_deref(), Some("hi"));
    assert_eq!(p.output, Some(OutputFormat::Json));
    assert_eq!(p.mode.as_deref(), Some("yolo"));
    assert_eq!(p.cwd, Some(PathBuf::from("/w")));
    assert_eq!(p.attach, vec!["a.png", "b.txt"]);
    assert!(p.desktop && p.verbose);
    // `-px`、`--prompt=` 与 `-cv` 组合、`--` 之后为位置参数、重复取最后一个。
    assert_eq!(parse(args(&["-px"])).unwrap().prompt.as_deref(), Some("x"));
    assert_eq!(
        parse(args(&["--prompt="])).unwrap().prompt.as_deref(),
        Some("")
    );
    let cv = parse(args(&["-cv"])).unwrap();
    assert!(cv.continue_session && cv.version);
    assert_eq!(
        parse(args(&["-p", "x", "--", "--json"])).unwrap().output,
        None
    );
    assert_eq!(
        parse(args(&["-p", "x", "-p", "y"]))
            .unwrap()
            .prompt
            .as_deref(),
        Some("y")
    );
    // 显式 --output-format 优先于 --json。
    assert_eq!(
        parse(args(&["-p", "x", "--json", "--output-format", "text"]))
            .unwrap()
            .output,
        Some(OutputFormat::Text)
    );
    assert_eq!(
        parse(args(&["-p", "x", "--json"])).unwrap().output,
        Some(OutputFormat::Json)
    );
}

#[test]
fn parse_errors_match_node_util_parse_args() {
    assert_eq!(
        err(&["-p"]).message,
        "Option '-p, --prompt <value>' argument missing"
    );
    assert_eq!(
        err(&["--mode"]).message,
        "Option '--mode <value>' argument missing"
    );
    assert_eq!(
        err(&["-p", "--json"]).message,
        "Option '-p' argument is ambiguous.\nDid you forget to specify the option argument for '-p'?\nTo specify an option argument starting with a dash use '--prompt=-XYZ' or '-p-XYZ'."
    );
    assert_eq!(
        err(&["--prompt", "-x"]).message,
        "Option '--prompt' argument is ambiguous.\nDid you forget to specify the option argument for '--prompt'?\nTo specify an option argument starting with a dash use '--prompt=-XYZ'."
    );
    assert_eq!(
        err(&["--json=1", "-p", "x"]).message,
        "Option '--json' does not take an argument"
    );
    let bogus = err(&["-p", "x", "--bogus"]);
    assert!(bogus.usage);
    assert_eq!(
        bogus.message,
        "Unknown option '--bogus'. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- \"--bogus\""
    );
    assert_eq!(err(&["foo"]).message, "Unknown command: foo");
}

#[test]
fn validation_errors_match_node_run() {
    let cases: [(&[&str], &str); 6] = [
        (
            &["-p", "x", "--output-format", "xml"],
            "--output-format must be one of text, json, stream-json (received: xml).",
        ),
        (
            &["-p", "x", "--mode", "bad"],
            "Unsupported --mode value: bad. Supported modes: build, edit, plan, yolo.",
        ),
        (
            &["-p", "x", "--resume", "a", "-c"],
            "--resume and --continue cannot be used together.",
        ),
        (
            &["-p", "x", "--locale", "fr"],
            "Unsupported --locale value: fr. Supported locales: en-US, zh-CN, auto.",
        ),
        (
            &["-p", "x", "--surface", "web"],
            "Unsupported --surface value: web. Supported surfaces: terminal, desktop.",
        ),
        (
            &["-p", "x", "--target", "t"],
            "--target is not supported by the Rust runtime.",
        ),
    ];
    for (argv, message) in cases {
        let error = err(argv);
        assert_eq!(error.message, message);
        assert!(!error.usage, "{message}");
    }
    assert_eq!(
        err(&["-p", "x", "--output-format", "stream-json"]).message,
        "--output-format stream-json is not supported by the Rust runtime yet."
    );
}

#[test]
fn disallowed_tools_are_greedy_and_normalized() {
    let p = parse(args(&[
        "-p",
        "x",
        "--disallowed-tools",
        "Bash(rm -rf, x),Write",
        "web_search",
        "--disallowedTools=Read",
        "--verbose",
    ]))
    .unwrap();
    assert_eq!(
        p.disallowed_tools,
        vec!["Bash(rm -rf, x)", "Write", "WebSearch", "Read"]
    );
    assert!(p.verbose);
    let error = err(&["-p", "x", "--disallowed-tools"]);
    assert_eq!(
        error.message,
        "--disallowed-tools requires at least one tool."
    );
    assert!(error.usage);
}
