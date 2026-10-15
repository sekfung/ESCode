//! escode-usb-capture：Linux usbmon → pcapng 抓包 helper（docs/specs/usb-agent-tools.md §4.1）。
//!
//! 分层：`pcapng` / `usbmon_bin` 里的纯逻辑（64 字节头解析、事件归一化、块写入、批处理计数）
//! 与 `usbmon_bin` 里 unsafe 的 ioctl+mmap 薄层分开，前者被 tests/ 全面覆盖，后者需要真机复核
//! （见 README「未在真机验证的部分」）。`probe` 只做能力探测，`install_privileges` 只做固定的四步授权。
//!
//! ⚠️ 其中 `usbmon_bin` 的 ioctl + mmap 路径未在真机验证（开发环境没有加载 usbmon、没有 /dev/usbmon*、没有 root）。

pub mod error;
#[cfg(unix)]
pub mod install_privileges;
pub mod pcapng;
pub mod probe;
pub mod usbmon_bin;
