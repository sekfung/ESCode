---
name: usb-capture
description: Use when a USB device on this machine misbehaves and the answer is on the wire — a smart-card / ID-card reader that will not read (CCID, APDU, PC/SC), a keyboard, mouse, gamepad or other HID device with wrong or missing input, a USB drive or USB-SATA disk that fails to read, write or mount (mass storage, SCSI), or a device that does not enumerate at all. Also use when the user hands over a .pcapng / .pcap USB capture to analyze. Not for serial/UART ports (use the serial-port skill) and not for network traffic (use the REA tshark path).
---

# USB capture and protocol analysis

Three tools form one funnel: **`usb_probe`** (what can this machine do, what is plugged in, what files exist)
→ **`usb_capture`** (record a slice now, needs approval) → **`usb_decode`** (turn a capture file into protocol messages).

## Which tool for which situation

| The user's problem | Call | Then |
| --- | --- | --- |
| Card / ID / bank card reader does not read, APDU fails | `usb_probe` to find the CCID device | `usb_capture` → `usb_decode(protocol=ccid)` |
| Keyboard, mouse, stick, custom HID behaves wrong, wrong key codes | `usb_probe` (or a capture the user already has) | `usb_decode(protocol=hid)` |
| USB drive / SSD fails to read, write or mount | `usb_probe` | `usb_capture` → `usb_decode(protocol=msc)` |
| User already has a `.pcapng` / `.pcap` | `usb_decode(view=summary)` directly | follow up with the protocol it reports |
| Device never enumerates, reconnects in a loop, drops out | `usb_capture` (ask the user to replug during the window) | `usb_decode(view=summary)` |
| This machine cannot capture at all (macOS, no permission, no driver) | `usb_probe` — it returns the exact `remediation` | hand that remediation to the user; ask for a capture file from another machine |
| Unknown / vendor-specific protocol | `usb_decode(protocol=auto, view=summary)` | `usb_decode(protocol=raw, view=timeline)` |

Rules that follow from the table:

1. **Probe before capturing.** If `usb_probe` reports the backend as unavailable or the privilege as denied, do **not** call `usb_capture`. Report its remediation text to the user verbatim — it is the exact command they need.
2. **A file beats a capture.** If the user already has a capture, skip `usb_capture` entirely.
3. **The interface class decides the protocol**, not a guess: read it from `usb_probe`'s device list (`smart-card-ccid` → `ccid`, `hid` → `hid`, `mass-storage` → `msc`) or from the descriptors inside the capture. Only use `protocol=auto` when neither is available.
4. **Summary first, then page.** `summary` is cheap and tells you devices, protocol spread and where the anomalies are. Only then pull `timeline` pages with `cursor`, and `message` for a single message's bytes.
5. **Never hand raw hex back as the answer.** Conclusions must be in protocol terms: APDU command names and `SW1SW2`, CBW/CDB/CSW and sense key/ASC, HID field names.

## Capturing (needs the user's approval)

`usb_capture` records a bounded slice on one bus. Before calling it, tell the user what you are about to record and
for how long, and ask them to reproduce the failure — or, when the problem is enumeration, to **unplug and replug the
device** — during that window. Only the traffic that crosses the bus while the capture runs lands in the file:
descriptors (VID:PID, interface class, HID report descriptor) only show up if the device was enumerated during the capture.

Keep `durationMs` as short as the reproduction allows (10 s is a good default, 60 s is the ceiling) and pass a `label`
such as `acr1252-select-fail` so the file is recognizable later.

## Decoding: what to look at

**CCID / card reader (`protocol=ccid`)** — one line per card exchange: power-on with its ATR, then each APDU with
the command name and the response status word. The failing part is nearly always the `SW1SW2` or a slot error:

| Status | Meaning |
| --- | --- |
| `9000` | success |
| `61xx` | more data available — send GET RESPONSE (`00 C0 00 00 xx`) |
| `6Cxx` | wrong Le — retry with `xx` as the length |
| `6300` / `63Cx` | verification failed / counter is `x` |
| `6982` | security status not satisfied (do the VERIFY / authentication first) |
| `6A82` / `6A83` | file / record not found (wrong AID selection or wrong file) |
| `6D00` / `6E00` | INS / CLA not supported by this card |
| `6700` | wrong length |

Also watch for card-removal (`RDR_to_PC_SlotStatus` without an error means the card sits in the slot but is not
activated) and for `bError` on the slot status message.

**HID (`protocol=hid`)** — the decoder prints each report decoded against the report descriptor that was in the
capture (button bits, axes, key codes). If it says the report descriptor is missing, the fix is a new capture: the
descriptor is read when the device is enumerated, so ask the user to replug during the capture window. Compare the
decoded field layout with what the device should send — a mismatch in report length or report ID is the usual root
cause of "the key code is wrong" or "one axis is stuck".

**Mass storage (`protocol=msc`)** — each logical command shows as CBW → data → CSW. Look at the CDB opcode (what the
host asked), the CSW `status` (0 ok, 1 failed, 2 phase error), `dCSWResidue` (bytes not transferred), and the
`REQUEST SENSE` result that follows a failure: the sense key (`MEDIUM ERROR`, `NOT READY`, `ILLEGAL REQUEST`,
`DATA PROTECT`, `UNIT ATTENTION`) plus the ASC/ASCQ pair is what tells the user whether the medium, the cabling or
the command is at fault.

## When capture is not available

Live capture ships **with ESCode itself**: `usb_probe` lists the capture backends it found, the first entry is the
built-in one (`builtin`), and `dumpcap` (from a Wireshark the user happens to have) is only a fallback. So the usual
situation is not "install a tool", it is "grant permission once":

- **Linux**: the built-in helper needs read access to `/dev/usbmonN`. If `usb_probe` reports `permission`, the fix is
  one system authorization — run the `remediation` it prints (an `escode-usb-capture --install-privileges` command via
  `pkexec`), which the user has to approve in a system dialog. It is permanent, so ask once, not every capture. If the
  usbmon kernel module is not loaded, the remediation asks for `sudo modprobe usbmon`.
- **Windows**: the capture service and the USBPcap driver are installed with ESCode. If `usb_probe` says the built-in
  backend is missing, the installation predates this feature — the user should reinstall with the current installer.
- **macOS**: there is no live capture and nothing to install. Ask the user for a `.pcapng` captured on a Linux/Windows
  machine or with a hardware analyzer instead.

Whatever the case, the **offline path always works**: `usb_decode` reads any capture file the user brings, from
Wireshark, USBPcap, usbmon or a hardware analyzer — and it never needs a privilege or an external tool. When a capture
is impossible right now, say so plainly and ask for a file; do not invent traffic.

## Context discipline

Captures have tens of thousands of packets. Never paste a whole timeline into the conversation: `summary` is a few
lines, `timeline` defaults to 200 lines and tells you the `cursor` for the next page, `message` shows one message
with at most `payloadBytes` of body. Quote only the few messages that support your conclusion, and keep them short.
