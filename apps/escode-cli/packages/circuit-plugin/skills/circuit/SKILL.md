---
name: circuit
description: Use when the user asks to read, explain, review or debug a schematic or PCB (KiCad, netlist, BOM, PDF) in the ESCode Desktop app — understanding a circuit, checking connections, tracing a net, or comparing a design against firmware pin definitions.
---

# Circuit analysis

The `mcp__circuit__*` tools read the **Circuit IR**: one structured, Host-owned description of the board that the user also sees in the circuit panel of this ESCode window. The IR is the only source of truth — never conclude anything about the design from a raw file or an image on your own when the IR is available.

## Workflow

1. **`circuit_parse`** with the workspace-relative `path` of the source file (`.kicad_sch`, `.kicad_pcb`, netlist, BOM). It returns `irId`, `revision` and a summary (component/net/block counts, diagnostics).
   - Re-parsing a file that already has an IR **must** pass the current `baseRevision`.
   - `stale: true` means the source file changed on disk: re-parse with `baseRevision` before trusting the IR.
2. **`circuit_get`** with `irId` to read the summary, the diagnostics list and how many elements are low-confidence. Do this before explaining anything.
3. **`circuit_query`** as needed:
   - `by: "ref"` → one component (pins, footprint, part number, the net of each pin); omit `ref` to page through every component (the panel uses the same read path for its BOM).
   - `by: "net"` with `netName` (or `netId`) → which pins/net names belong to that net; omit both to list every net.
   - `by: "block"` → the functional blocks the user/agent defined; omit the filter to list them all.
   - `by: "trace"` with `componentId` (+ optional `pin`) → everything reachable on that pin's net.
   - Results are paged: when the answer contains `nextCursor`, pass it back as `cursor` to read the next page (the cursor is opaque — never build or edit one).
4. **`circuit_check`** for a design review and **`circuit_compare_firmware`** for hardware/firmware bring-up (§5). Both are read-only and both page their results with `nextCursor` → `cursor`.

## Rule checks and firmware comparison

- **`circuit_check`** runs the six deterministic built-in rules (§6) over the IR: `unconnected-pin`, `single-pin-net`, `undriven-power`, `i2c-missing-pullup`, `missing-decoupling`, `net-name-conflict`. It returns `config` (the `.escode/circuit-rules.json` status: `default` / `applied` / `invalid`) and `items` with `ruleId`, `severity`, the involved component/net ids and `lowConfidence`.
  - **Present rule findings and your own review comments separately** (§2 rule 5): report each rule hit as a fact with its rule id and severity, and put your own opinions in a clearly labelled "suggestions" part. Never present a model opinion as a rule result.
  - When `config.status` is `invalid`, say that the workspace rule config was ignored and the default configuration was used (quote `config.reason`); `default` means no config file was found.
  - Items touching low-confidence elements carry `lowConfidence: true` — call those out as with any other low-confidence data.
- **`circuit_compare_firmware`** is the software/hardware bring-up check (§7): pass the IR and workspace-relative firmware files (Zephyr devicetree `.dts`/`.dtsi`/`.overlay`, STM32CubeMX `.ioc`, or C headers with `_PIN` + `_GPIO_PORT` macros) and it returns differences as `pin-mismatch`, `firmware-only`, `schematic-only` or `peripheral-conflict`, each with pin, net, firmware symbol, file and line.
  - Report each difference with the firmware `file`/`line` and the schematic ref/net so the user knows which side to change; a difference is not by itself proof of which side is wrong.
  - Only the constructs listed in §7.1 are parsed. Pin assignments scattered through Arduino/PlatformIO code are not parsed — if you infer them yourself, label the result as a low-confidence suggestion.
  - `files` must stay within the workspace; an unsupported extension answers `unsupported_format`, an IR without recognizable MCU pin names answers `invalid_ir`.

## Explaining a circuit

- **Always cite reference designators and net names** (`U3`, `R12`, `NET:I2C1_SDA`). A conclusion without refs and nets is not useful.
- Split the explanation by function (power, MCU, interfaces, protection) and describe the signal flow between blocks; use `circuit_query` to confirm each hop instead of assuming.
- **Say which conclusions depend on low-confidence data.** Any element whose `origin.by` is `recognition` (or that `circuit_get` counts as low-confidence) must be called out, including in the diagnostics. Deterministic parses are confidence 1 and need no caveat.
- Rule-check results and firmware comparisons are separate from your own review comments; label model opinions as suggestions.

## Exporting

- `circuit_export` writes a read-only `.circuit.json` snapshot into the workspace. It **requires the user's approval**, and it is a derived artifact: it is never read back, and editing it does not change the IR. Ask before calling it, and pass a path ending in `.circuit.json`.
- To open the interactive panel for a result, put a reference on its own line in your reply: `::circuit{"irId":"…","title":"…","componentCount":53,"netCount":39}`. Do not put it inside a code block.
  - Order matters: `circuit_parse` (or a recognition submission) → `circuit_get` → the reference card. Take `componentCount` and `netCount` from the `circuit_get` summary and always include them (the same counts the panel shows) — e.g. `::circuit{"irId":"ir:1","title":"电源板","componentCount":53,"netCount":39}`.
  - These two counts feed the static fallback card on Web and mobile, where the circuit panel is not available; the Desktop panel reads the IR itself and does not depend on them.

## Input that is not text

- Vector PDFs and screenshots cannot be parsed by `circuit_parse`. Recognition from images is a separate capability and may not be available in this build — if the tools do not offer it, tell the user instead of guessing element values.
- Before sending any image or PDF content to a model for recognition, tell the user what will be sent and why.

## When tools answer `unavailable`

Circuit tools only work in the ESCode Desktop app with this workspace open (the IR lives in its Host). `[unavailable]` means exactly that: ask the user to open the workspace in ESCode Desktop. Do not try to reconstruct the IR by editing files yourself — the IR has a single owner and a single write path.
