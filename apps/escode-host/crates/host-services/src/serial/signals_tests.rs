//! 复位脉冲时序（`rust-u2-serial.md` §12.1）。
use super::*;

#[test]
fn esp32_pulse_matches_the_esptool_sequence() {
    let steps = pulse_steps(
        Pulse::Esp32,
        Signals {
            dtr: true,
            rts: true,
        },
    );
    assert_eq!(steps.len(), 3, "ESP32 脉冲三步");
    assert_eq!(
        steps[0].signals,
        Signals {
            dtr: false,
            rts: true
        }
    );
    assert_eq!(steps[0].hold, Duration::from_millis(100));
    assert_eq!(
        steps[1].signals,
        Signals {
            dtr: true,
            rts: false
        }
    );
    assert_eq!(steps[1].hold, Duration::from_millis(50));
    assert_eq!(
        steps[2].signals,
        Signals {
            dtr: false,
            rts: false
        }
    );
    assert_eq!(steps[2].hold, Duration::ZERO);
}

#[test]
fn arduino_pulse_toggles_dtr_and_keeps_rts() {
    let previous = Signals {
        dtr: false,
        rts: false,
    };
    let steps = pulse_steps(Pulse::Arduino, previous);
    assert_eq!(steps.len(), 2, "Arduino 脉冲两步");
    assert_eq!(
        steps[0].signals,
        Signals {
            dtr: true,
            rts: false
        }
    );
    assert_eq!(steps[0].hold, Duration::from_millis(100));
    assert_eq!(
        steps[1].signals,
        Signals {
            dtr: false,
            rts: false
        }
    );
    assert_eq!(steps[1].hold, Duration::ZERO);

    // RTS 由脉冲前的状态决定，不受 DTR 影响。
    let steps = pulse_steps(
        Pulse::Arduino,
        Signals {
            dtr: true,
            rts: true,
        },
    );
    assert!(steps[0].signals.rts);
    assert!(steps[1].signals.rts);
}

#[test]
fn pulse_names_are_parsed_like_the_typescript_parameter() {
    assert_eq!(Pulse::from_name("esp32"), Some(Pulse::Esp32));
    assert_eq!(Pulse::from_name("arduino"), Some(Pulse::Arduino));
    // TS 的 `if (pulse === "esp32") … else …`：其它非空值走 Arduino；空串是 falsy（无脉冲）。
    assert_eq!(Pulse::from_name("custom"), Some(Pulse::Arduino));
    assert_eq!(Pulse::from_name(""), None);
    assert_eq!(
        DEFAULT_SIGNALS.to_json(),
        serde_json::json!({"dtr": true, "rts": true})
    );
}
