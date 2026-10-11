# Restore Legacy Sessions Plugin

ESCode official plugin for restoring old ACP-era ESCode session snapshots into the new task index and CLI session DB.

This plugin is bundled as `restore-legacy-sessions@zcode-plugins-official` and is disabled by default. Enable it only when you need to inspect or restore old session data.

## Enable

```sh
escode plugins enable restore-legacy-sessions
```

Or inside a ESCode session:

```text
/plugins enable restore-legacy-sessions
```

Changes apply to new sessions. After enabling, use:

```text
/restore-legacy-sessions
```

## Data Boundaries

- Default source: `~/.escode/v2/sessions`
- Task index destination: `~/.escode/v2/tasks-index.sqlite`
- CLI session DB destination: `~/.escode/cli/db/db.sqlite`

The restore scripts create timestamped DB backups before writing. Restored ACP-era conversations are persisted as normal `glm` ESCode Agent history and do not write `migration_source`.

## Development

This is a skill-only / command-only plugin. It has no MCP server and no build step.
