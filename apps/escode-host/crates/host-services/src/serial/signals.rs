//! DTR/RTS 输出与复位脉冲时序（S4）：TS `serialSignals.ts` 的 Rust 对应物。
//!
//! 脉冲每一步给出目标信号与保持时间；最后一步之后由会话恢复脉冲前的状态（不在这里展开）。

use std::time::Duration;

use serde_json::{Value, json};

/// TS `SerialSignals`。打开串口后的初值 DTR=1、RTS=1（与 serialport 默认一致）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signals {
    pub dtr: bool,
    pub rts: bool,
}

pub const DEFAULT_SIGNALS: Signals = Signals {
    dtr: true,
    rts: true,
};

impl Signals {
    pub fn to_json(self) -> Value {
        json!({"dtr": self.dtr, "rts": self.rts})
    }
}

/// TS `SerialSignalPulse`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pulse {
    Esp32,
    Arduino,
}

impl Pulse {
    /// RPC 参数：`"esp32"`；其它非空字符串按 Arduino 处理（TS 的 `else` 分支）。
    pub fn from_name(name: &str) -> Option<Self> {
        if name.is_empty() {
            return None;
        }
        Some(if name == "esp32" {
            Self::Esp32
        } else {
            Self::Arduino
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PulseStep {
    pub signals: Signals,
    pub hold: Duration,
}

/// TS `serialPulseSteps`：
/// - ESP32（esptool 经典复位）：DTR=0,RTS=1 → 100ms → DTR=1,RTS=0 → 50ms → DTR=0；
/// - Arduino：DTR=1 保持 100ms → DTR=0（RTS 维持原状）。
pub fn pulse_steps(pulse: Pulse, previous: Signals) -> Vec<PulseStep> {
    match pulse {
        Pulse::Esp32 => vec![
            PulseStep {
                signals: Signals {
                    dtr: false,
                    rts: true,
                },
                hold: Duration::from_millis(100),
            },
            PulseStep {
                signals: Signals {
                    dtr: true,
                    rts: false,
                },
                hold: Duration::from_millis(50),
            },
            PulseStep {
                signals: Signals {
                    dtr: false,
                    rts: false,
                },
                hold: Duration::ZERO,
            },
        ],
        Pulse::Arduino => vec![
            PulseStep {
                signals: Signals {
                    dtr: true,
                    rts: previous.rts,
                },
                hold: Duration::from_millis(100),
            },
            PulseStep {
                signals: Signals {
                    dtr: false,
                    rts: previous.rts,
                },
                hold: Duration::ZERO,
            },
        ],
    }
}

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;
