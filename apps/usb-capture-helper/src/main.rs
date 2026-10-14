//! 命令行入口（docs/specs/usb-agent-tools.md §4.1）：手写参数解析，stdout 恒为一行 JSON。
//!
//! 三个子命令互斥：`--probe`、抓包（`--bus N --duration-ms M --max-bytes B --output PATH`）、
//! `--install-privileges`（只在 pkexec / root 下调用）。失败时打印 `{ ok: false, error: {...} }`
//! 并返回非 0 退出码（invalidInput=2 / permission=3 / no-interface=4 / busy=5 / io=6）。

use std::path::PathBuf;
use std::process::ExitCode;

use escode_usb_capture::error::HelperError;
use escode_usb_capture::probe;
use escode_usb_capture::usbmon_bin::{
    self, CaptureOutcome, CaptureRequest, MAX_BUS, MAX_DURATION_MS, MAX_MAX_BYTES, MIN_BUS,
    MIN_DURATION_MS, MIN_MAX_BYTES,
};
use serde_json::{Value, json};

/// 与 spec §3.3 的工具级默认值一致（helper 允许省略，插件始终显式传参）。
const DEFAULT_DURATION_MS: u64 = 10_000;
const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024;

const USAGE: &str = "\
escode-usb-capture —— ESCode USB 抓包 helper（Linux usbmon → pcapng）

用法：
  escode-usb-capture --probe
  escode-usb-capture --bus N --duration-ms M --max-bytes B --output PATH.pcapng
  escode-usb-capture --install-privileges        # 需要 pkexec 或 root

参数：
  --bus N          总线号（1–255；对应 /dev/usbmonN）
  --duration-ms M  抓包时长，1–60000（默认 10000）
  --max-bytes B    抓包体积上限，1–536870912（默认 64 MiB）
  --output PATH    输出文件，必须以 .pcapng 结尾；先写 PATH.partial，正常收尾后改名
  --probe          只探测能力与权限，不抓包
  --install-privileges  创建 escode-usb 组、写 udev 规则、加载 usbmon、修正现有节点权限
  -h, --help       打印本说明

输出：stdout 恒为一行 JSON；失败时 { ok: false, error: { code, message, remediation? } }，退出码非 0。
成功退出码 0；invalidInput=2、permission=3、no-interface=4、busy=5、io=6。";

enum Command {
    Probe,
    InstallPrivileges,
    Capture(CaptureRequest),
    Help,
}

enum Output {
    Json(Value),
    Usage,
}

fn main() -> ExitCode {
    // 信号处理器装不上只影响「被中断时能否优雅收尾」，不影响抓包本身，因此只警告不失败。
    if let Err(error) = usbmon_bin::install_signal_handlers() {
        eprintln!("警告：{error}");
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(Output::Json(value)) => {
            println!("{value}");
            ExitCode::SUCCESS
        }
        Ok(Output::Usage) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            let payload = json!({ "ok": false, "error": error.to_json() });
            println!("{payload}");
            ExitCode::from(error.code.exit_code())
        }
    }
}

fn run(args: &[String]) -> Result<Output, HelperError> {
    match parse(args)? {
        Command::Help => Ok(Output::Usage),
        Command::Probe => Ok(Output::Json(probe::probe().to_json())),
        Command::InstallPrivileges => {
            #[cfg(unix)]
            {
                escode_usb_capture::install_privileges::install_privileges()
                    .map(|result| Output::Json(result.to_json()))
            }
            #[cfg(not(unix))]
            {
                Err(HelperError::new(
                    escode_usb_capture::error::ErrorCode::NoInterface,
                    "特权步骤只实现了 Linux 的 udev / 用户组机制。",
                ))
            }
        }
        Command::Capture(request) => {
            usbmon_bin::run_capture(&request).map(|outcome| Output::Json(capture_json(&outcome)))
        }
    }
}

fn capture_json(outcome: &CaptureOutcome) -> Value {
    json!({
        "ok": true,
        "packets": outcome.packets,
        "bytes": outcome.bytes,
        "dropped": outcome.dropped,
        "skippedIsochronous": outcome.skipped_isochronous,
        "stoppedBy": outcome.stopped_by.as_str(),
        "output": outcome.output.to_string_lossy().to_string(),
        "bus": outcome.bus,
    })
}

fn parse(args: &[String]) -> Result<Command, HelperError> {
    let mut probe = false;
    let mut install = false;
    let mut help = false;
    let mut bus: Option<u32> = None;
    let mut duration_ms: Option<u64> = None;
    let mut max_bytes: Option<u64> = None;
    let mut output: Option<PathBuf> = None;

    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "--probe" => probe = true,
            "--install-privileges" => install = true,
            "--help" | "-h" => help = true,
            "--bus" => bus = Some(parse_bus(take_value(args, &mut index, "--bus")?)?),
            "--duration-ms" => {
                duration_ms = Some(parse_duration_ms(take_value(
                    args,
                    &mut index,
                    "--duration-ms",
                )?)?);
            }
            "--max-bytes" => {
                max_bytes = Some(parse_max_bytes(take_value(
                    args,
                    &mut index,
                    "--max-bytes",
                )?)?);
            }
            "--output" => {
                output = Some(PathBuf::from(take_value(args, &mut index, "--output")?));
            }
            other => {
                return Err(HelperError::invalid_input(format!("未知参数：{other}")));
            }
        }
        index += 1;
    }

    if help {
        return Ok(Command::Help);
    }
    let capture_requested =
        bus.is_some() || duration_ms.is_some() || max_bytes.is_some() || output.is_some();
    let selected = [probe, install, capture_requested]
        .iter()
        .filter(|flag| **flag)
        .count();
    match selected {
        0 => Err(HelperError::invalid_input(
            "缺少子命令：--probe、抓包参数（--bus/--output）或 --install-privileges。",
        )),
        1 if probe => Ok(Command::Probe),
        1 if install => Ok(Command::InstallPrivileges),
        1 => {
            let bus = bus.ok_or_else(|| HelperError::invalid_input("抓包需要 --bus N"))?;
            let output =
                output.ok_or_else(|| HelperError::invalid_input("抓包需要 --output PATH"))?;
            Ok(Command::Capture(CaptureRequest {
                bus,
                duration_ms: duration_ms.unwrap_or(DEFAULT_DURATION_MS),
                max_bytes: max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
                output,
            }))
        }
        _ => Err(HelperError::invalid_input(
            "一次只能使用一种子命令：--probe / 抓包参数 / --install-privileges 互斥。",
        )),
    }
}

/// 取 `--flag value` 里的 value；缺值或写成另一个 flag 都算参数错误。
fn take_value<'a>(
    args: &'a [String],
    index: &mut usize,
    name: &str,
) -> Result<&'a str, HelperError> {
    *index += 1;
    let value = args
        .get(*index)
        .ok_or_else(|| HelperError::invalid_input(format!("{name} 缺少取值")))?;
    if value.starts_with("--") {
        return Err(HelperError::invalid_input(format!(
            "{name} 缺少取值（拿到的是 {value}）"
        )));
    }
    Ok(value.as_str())
}

fn parse_bus(raw: &str) -> Result<u32, HelperError> {
    let bus = raw
        .parse::<u32>()
        .map_err(|_| HelperError::invalid_input(format!("--bus 必须是整数：{raw}")))?;
    if !(MIN_BUS..=MAX_BUS).contains(&bus) {
        return Err(HelperError::invalid_input(format!(
            "--bus 必须在 {MIN_BUS}–{MAX_BUS} 之间（0 是 usbmon 的聚合设备，不作为抓包目标）：{bus}"
        )));
    }
    Ok(bus)
}

fn parse_duration_ms(raw: &str) -> Result<u64, HelperError> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| HelperError::invalid_input(format!("--duration-ms 必须是整数：{raw}")))?;
    if !(MIN_DURATION_MS..=MAX_DURATION_MS).contains(&value) {
        return Err(HelperError::invalid_input(format!(
            "--duration-ms 必须在 {MIN_DURATION_MS}–{MAX_DURATION_MS} 之间：{value}"
        )));
    }
    Ok(value)
}

fn parse_max_bytes(raw: &str) -> Result<u64, HelperError> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| HelperError::invalid_input(format!("--max-bytes 必须是整数：{raw}")))?;
    if !(MIN_MAX_BYTES..=MAX_MAX_BYTES).contains(&value) {
        return Err(HelperError::invalid_input(format!(
            "--max-bytes 必须在 {MIN_MAX_BYTES}–{MAX_MAX_BYTES} 之间：{value}"
        )));
    }
    Ok(value)
}
