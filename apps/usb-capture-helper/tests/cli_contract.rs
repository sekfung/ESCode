//! CLI 契约测试：直接运行编译出的二进制。
//!
//! 覆盖三件事：参数与输出路径校验失败都返回 `invalidInput` 且不 panic、stdout 恒为一行 JSON；
//! 设备节点不存在时抓包返回 `no-interface` 且不留下半截文件；`--probe` 在无 usbmon 的机器上
//! 也必须成功返回（`available: false` + remediation），而不是报失败。

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_escode-usb-capture");

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Run {
    let output = Command::new(BIN).args(args).output().expect("执行 helper");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn parse_json(stdout: &str) -> Value {
    let lines: Vec<&str> = stdout.trim_end().lines().collect();
    assert_eq!(lines.len(), 1, "stdout 必须恰好一行 JSON，实际：{stdout}");
    serde_json::from_str(lines[0]).expect("stdout 必须是合法 JSON")
}

fn error_code(stdout: &str) -> String {
    let value = parse_json(stdout);
    assert_eq!(
        value["ok"],
        Value::Bool(false),
        "失败响应的 ok 必须是 false"
    );
    value["error"]["code"]
        .as_str()
        .expect("error.code 必须是字符串")
        .to_string()
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "escode-usb-capture-cli-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("创建测试临时目录");
    path
}

#[test]
fn invalid_bus_is_invalid_input() {
    let dir = temp_dir("invalid-bus");
    for bus in ["0", "abc", "256", "-1", "1.5", ""] {
        let output = dir.join("out.pcapng");
        let output_arg = output.to_str().expect("路径");
        let result = run(&["--bus", bus, "--output", output_arg]);
        assert_eq!(
            result.code,
            Some(2),
            "--bus {bus} 应退出码 2，stderr: {}",
            result.stderr
        );
        assert_eq!(error_code(&result.stdout), "invalidInput");
        assert!(!output.exists(), "--bus {bus} 不应创建任何文件");
        assert!(!dir.join("out.pcapng.partial").exists());
    }
}

#[test]
fn invalid_bounds_and_missing_values_are_invalid_input() {
    let dir = temp_dir("invalid-bounds");
    let output = dir.join("out.pcapng");
    let output = output.to_str().expect("路径");
    let cases: Vec<Vec<&str>> = vec![
        vec!["--bus", "1", "--duration-ms", "0", "--output", output],
        vec!["--bus", "1", "--duration-ms", "60001", "--output", output],
        vec!["--bus", "1", "--duration-ms", "abc", "--output", output],
        vec!["--bus", "1", "--max-bytes", "0", "--output", output],
        vec!["--bus", "1", "--max-bytes", "536870913", "--output", output],
        vec!["--bus", "1", "--max-bytes", "-5", "--output", output],
        vec!["--bus", "1", "--duration-ms", "--output", output],
        vec!["--bus", "1", "--output"],
        vec!["--bus"],
        vec!["--probe", "--bus", "1"],
        vec!["--probe", "--install-privileges"],
        vec!["--unknown-flag"],
        vec![],
    ];
    for args in cases {
        let result = run(&args);
        assert_eq!(
            result.code,
            Some(2),
            "参数 {args:?} 应退出码 2，stderr: {}",
            result.stderr
        );
        assert_eq!(error_code(&result.stdout), "invalidInput", "参数 {args:?}");
    }
}

#[test]
fn output_path_validation_rejects_bad_paths() {
    let dir = temp_dir("bad-output");

    // 非 .pcapng 后缀。
    let wrong_suffix = dir.join("capture.pcap");
    let result = run(&[
        "--bus",
        "1",
        "--output",
        wrong_suffix.to_str().expect("路径"),
    ]);
    assert_eq!(result.code, Some(2));
    assert_eq!(error_code(&result.stdout), "invalidInput");
    assert!(!wrong_suffix.exists());

    // 父目录不存在。
    let missing_parent = dir.join("no-such-dir").join("capture.pcapng");
    let result = run(&[
        "--bus",
        "1",
        "--output",
        missing_parent.to_str().expect("路径"),
    ]);
    assert_eq!(result.code, Some(2));
    assert_eq!(error_code(&result.stdout), "invalidInput");

    // 输出本身就是符号链接：拒绝（避免被参数化路径写到别处）。
    let target = dir.join("real-target.bin");
    fs::write(&target, b"do-not-touch").expect("写目标文件");
    let link = dir.join("link.pcapng");
    std::os::unix::fs::symlink(&target, &link).expect("创建符号链接");
    let result = run(&["--bus", "1", "--output", link.to_str().expect("路径")]);
    assert_eq!(result.code, Some(2));
    assert_eq!(error_code(&result.stdout), "invalidInput");
    assert_eq!(
        fs::read(&target).expect("读目标文件"),
        b"do-not-touch",
        "不得跟随符号链接写入"
    );

    // 父目录是符号链接：同样拒绝。
    let real_dir = dir.join("real-dir");
    fs::create_dir_all(&real_dir).expect("创建真目录");
    let linked_dir = dir.join("linked-dir");
    std::os::unix::fs::symlink(&real_dir, &linked_dir).expect("创建目录符号链接");
    let via_link = linked_dir.join("capture.pcapng");
    let result = run(&["--bus", "1", "--output", via_link.to_str().expect("路径")]);
    assert_eq!(result.code, Some(2));
    assert_eq!(error_code(&result.stdout), "invalidInput");
    assert!(!real_dir.join("capture.pcapng").exists());
    assert!(!real_dir.join("capture.pcapng.partial").exists());
}

#[test]
fn probe_reports_capability_without_failing() {
    let result = run(&["--probe"]);
    assert_eq!(
        result.code,
        Some(0),
        "probe 不应失败，stderr: {}",
        result.stderr
    );
    let value = parse_json(&result.stdout);
    assert_eq!(value["ok"], Value::Bool(true));
    assert!(value["platform"].is_string());
    assert!(value["usbmonModuleLoaded"].is_boolean());
    assert!(value["available"].is_boolean());
    let privilege = value["privilege"].as_str().expect("privilege 是字符串");
    assert!(
        privilege == "ok" || privilege == "denied",
        "privilege = {privilege}"
    );
    assert!(value["interfaces"].is_array());
    // 不可用时必须给出中文 remediation（插件原样转述给用户）。
    if value["available"] == Value::Bool(false) {
        assert!(value["reason"].is_string());
        assert!(value["remediation"].is_string());
    }
}

#[test]
fn missing_interface_is_reported_without_partial_file() {
    if std::env::consts::OS != "linux" {
        return;
    }
    // /dev/usbmon9 在绝大多数机器上不存在；存在时跳过（本测试不抓真实流量）。
    if std::path::Path::new("/dev/usbmon9").exists() {
        return;
    }
    let dir = temp_dir("missing-interface");
    let output = dir.join("bus9.pcapng");
    let result = run(&[
        "--bus",
        "9",
        "--duration-ms",
        "100",
        "--max-bytes",
        "1048576",
        "--output",
        output.to_str().expect("路径"),
    ]);
    assert_eq!(
        result.code,
        Some(4),
        "应返回 no-interface（4），stderr: {}",
        result.stderr
    );
    assert_eq!(error_code(&result.stdout), "no-interface");
    assert!(!output.exists());
    assert!(
        !dir.join("bus9.pcapng.partial").exists(),
        "打开设备失败不得留下 .partial"
    );
}
