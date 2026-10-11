---
name: serial-port
description: Use when the user asks to talk to a device over a serial port / UART / COM port in the ESCode Desktop app — flashing firmware and checking boot logs, sending AT commands, or verifying device responses.
---

# Serial port

The `mcp__serial__*` tools share **one** serial session with the user's Serial Port panel in this ESCode window. Everything you send or receive also appears in that panel, tagged with your session.

## Rules

- Start with `serial_list` to see available ports and whether a port is already open.
- If the user already has a port open with different settings, do **not** try to switch it. Ask the user.
- `serial_open`, `serial_write` and `serial_close` need the user's approval. Explain why before calling them.
- Closing the port also disconnects the user's panel. Only close when you opened it or the user asked.

## Typical flow: flash, then wait for boot

1. Note the cursor before the action that resets the device:
   `serial_read` with no `sinceSeq` → returns `lastSeq` (and no data).
2. Flash or reset the device with your usual tools.
3. `serial_wait_for` with `pattern` (for example `"boot ok|ready"`), `sinceSeq` = the cursor from step 1, and a `timeoutMs` that fits the device (max 120000).
4. If it times out, read the returned `tail` to see where the device stopped, then decide what to do.

## Sending commands

- Text: `serial_write` with `data: "AT+GMR"` and `lineEnding: "crlf"` when the device expects CRLF.
- Raw bytes: `serial_write` with `encoding: "hex"` and `data: "AA 55 01 00"`. No line ending is added.
- After writing, use `serial_wait_for` (or `serial_read` from the `seq` returned by the write) to check the response.

## Reading output

- Keep the `lastSeq` from each `serial_read` and pass it back as `sinceSeq` to read only new data.
- `truncated: true` means more data is waiting; read again from the returned `lastSeq`.
- `evicted: true` means some data between your cursor and now was dropped from the 1 MiB buffer or cleared by the user.
- Use `encoding: "gbk"` for Chinese firmware logs that are not UTF-8, and `encoding: "hex"` for binary protocols.
