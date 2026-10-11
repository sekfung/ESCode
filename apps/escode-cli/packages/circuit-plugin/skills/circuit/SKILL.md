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

## Explaining a circuit

- **Always cite reference designators and net names** (`U3`, `R12`, `NET:I2C1_SDA`). A conclusion without refs and nets is not useful.
- Split the explanation by function (power, MCU, interfaces, protection) and describe the signal flow between blocks; use `circuit_query` to confirm each hop instead of assuming.
- **Say which conclusions depend on low-confidence data.** Any element whose `origin.by` is `recognition` (or that `circuit_get` counts as low-confidence) must be called out, including in the diagnostics. Deterministic parses are confidence 1 and need no caveat.
- Rule-check results and firmware comparisons are separate from your own review comments; label model opinions as suggestions.

## Exporting

- `circuit_export` writes a read-only `.circuit.json` snapshot into the workspace. It **requires the user's approval**, and it is a derived artifact: it is never read back, and editing it does not change the IR. Ask before calling it, and pass a path ending in `.circuit.json`.
- To open the interactive panel for a result, put a reference on its own line in your reply: `::circuit{"irId":"…","title":"…"}`. Do not put it inside a code block.

## Input that is not text

- Vector PDFs and screenshots cannot be parsed by `circuit_parse`. Recognition from images is a separate capability and may not be available in this build — if the tools do not offer it, tell the user instead of guessing element values.
- Before sending any image or PDF content to a model for recognition, tell the user what will be sent and why.

## When tools answer `unavailable`

Circuit tools only work in the ESCode Desktop app with this workspace open (the IR lives in its Host). `[unavailable]` means exactly that: ask the user to open the workspace in ESCode Desktop. Do not try to reconstruct the IR by editing files yourself — the IR has a single owner and a single write path.
