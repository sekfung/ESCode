# REA

Reverse engineer binaries, applications and runtime behavior — the [REA](https://github.com/morluto/rea) MCP
server plus its matching investigation skill, packaged as an ESCode plugin.

REA and its skill are by [morluto](https://github.com/morluto/rea) (MIT). This plugin is ESCode's integration
glue: it pins a REA version, launches it, and ships the matching skill.

## Requirements

- **Node.js and npm on `PATH`.** REA needs Node `^22.19.0 || ^24.11.0 || >=26.0.0` (23 and 25 are excluded
  upstream). ESCode does not bundle REA or its runtime; the launcher runs
  `npx -y rea-agents@6.4.0 mcp` and REA is fetched from the npm registry on first use.
- **Analysis engines depend on the target.** JavaScript/Electron, .NET and artifact/package inspection work
  with no extra tooling. Deep native decompilation needs a provider (Hopper, Ghidra or IDA). Several targets
  are Linux-only upstream (ELF layout, EVM bytecode, recorded crashes, firmware, packet capture) — REA reports
  its own platform limits, ESCode does not probe for them.

## What the plugin provides

- One MCP server named `rea`, started per session and stopped with it.
- All of REA's tools, advertised with REA's `compact` input-schema profile. The full profile is roughly
  496 KB of tool definitions (~120k tokens) and some providers reject it outright; `compact` keeps the same
  tools and server-side validation while cutting the advertised payload to roughly 290 KB (~68k tokens).
  The bundled skill carries the guidance that the compact profile trims from the schemas.
- The `reverse-engineer-anything` skill, vendored unmodified. It is scoped to this plugin: disabling the
  plugin removes both the tools and the skill.

## Failure modes

Everything goes through ESCode's ordinary MCP status display. If Node is missing or too old, the server
reports a process-start failure.

## Diagnostics

Both host runtimes hide REA's own stderr: the Rust runtime discards it outright (`Stdio::null()`) and the
Node runtime only keeps a sanitized tail in ESCode's log. The launcher therefore writes into this plugin's
data directory (the per-plugin directory ESCode manages under its storage):

- `rea-runtime-report.txt` — resolved runtime root, REA version, Node version and mode, schema profile, and
  which optional engine env vars are set. Overwritten on every start; this is where to look first when a
  capability "isn't there".
- `rea-mcp.log` — a session marker per start plus REA's own stderr: configuration validation errors,
  missing providers, platform limits. Capped at about 1 MB per session so it cannot grow without bound.

REA reports its own platform restrictions per target (several targets are Linux-only upstream), so the log
is also the place to confirm *why* a given capability is unavailable.
