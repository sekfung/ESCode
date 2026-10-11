import type { SerialSignalPulse, SerialSignals } from "./serial.js";

export const DEFAULT_SERIAL_SIGNALS: SerialSignals = { dtr: true, rts: true };

/** 一步信号输出及其保持时间；最后一步之后恢复脉冲前的状态。 */
export interface SerialSignalStep {
  signals: SerialSignals;
  holdMs: number;
}

/**
 * 复位脉冲时序（docs/specs/serial-port-debugger-phase3.md 第 2 节）：
 * - ESP32（esptool 经典复位）：DTR=0,RTS=1 → 100ms → DTR=1,RTS=0 → 50ms → DTR=0；
 * - Arduino：DTR=1 保持 100ms → DTR=0（RTS 维持原状）。
 */
export function serialPulseSteps(
  pulse: SerialSignalPulse,
  previous: SerialSignals,
): SerialSignalStep[] {
  if (pulse === "esp32") {
    return [
      { signals: { dtr: false, rts: true }, holdMs: 100 },
      { signals: { dtr: true, rts: false }, holdMs: 50 },
      { signals: { dtr: false, rts: false }, holdMs: 0 },
    ];
  }
  return [
    { signals: { dtr: true, rts: previous.rts }, holdMs: 100 },
    { signals: { dtr: false, rts: previous.rts }, holdMs: 0 },
  ];
}

export function isDefaultSerialSignals(signals: SerialSignals): boolean {
  return signals.dtr === DEFAULT_SERIAL_SIGNALS.dtr && signals.rts === DEFAULT_SERIAL_SIGNALS.rts;
}
