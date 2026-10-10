// 生成 shell_snapshot_corpus.json 的 TS oracle 脚本（docs/specs/rust-bash-shell-snapshot.md）：
// node --import tsx apps/escode-cli-rust/crates/domain/src/shell_snapshot_corpus.gen.ts
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ShellInitSnapshotManager } from "../../../../escode-cli/packages/adapters/src/exec/shell-init-snapshot.ts";
import { createCwdCapturePlan } from "../../../../escode-cli/packages/adapters/src/exec/cwd-capture.ts";
import { appendBashCwdStderrSuffix } from "../../../../escode-cli/packages/core/src/tool/handlers/bash-cwd-policy.ts";
import { gitBashPathToWindowsPath } from "../../../../escode-cli/packages/contracts/src/path/git-bash.ts";

const scripts: unknown[] = [];
for (const shell of ["/bin/bash", "/usr/bin/zsh", "/bin/sh"]) {
  for (const exists of [true, false]) {
    const home = mkdtempSync(join(tmpdir(), "snap-home-"));
    const config = shell.includes("zsh") ? ".zshrc" : shell.includes("bash") ? ".bashrc" : ".profile";
    if (exists) writeFileSync(join(home, config), "alias ll='ls -l'\n");
    let script = "";
    const manager = new ShellInitSnapshotManager({
      execFile: async (_file, args) => {
        script = args[2]!;
        return { stdout: "", stderr: "" };
      },
    });
    await manager.getOrCreate({
      env: { HOME: home, PATH: "/usr/bin:/bin:/it's here" },
      rootDir: join(home, "root"),
      shellDialect: "posix",
      shellPath: shell,
    });
    const snapshotPath = /^SNAPSHOT_FILE='(.*)'$/m.exec(script)![1]!.replaceAll("'\\''", "'");
    scripts.push({ shell, exists, configPath: join(home, config), pathValue: "/usr/bin:/bin:/it's here", snapshotPath, script });
  }
}
const capture = (dialect: "posix" | "cmd", command: string) =>
  createCwdCapturePlan(
    { command: { mode: "shell", command, shellProfile: "posix-bash" }, captureCwdAfterSuccess: true } as never,
    { dialect, platform: dialect === "cmd" ? "win32" : "linux" },
  );
const captures = (["posix", "cmd"] as const).map((dialect) => {
  const plan = capture(dialect, "echo 'hi'\nfalse");
  return { dialect, cwdFile: plan.cwdFilePath, command: plan.command.command };
});
const gitBash = ["/c/Users/a b/x", "/cygdrive/d/work", "/c", "//server/share/x", "/tmp/x", "/c/"].map((value) => ({
  value,
  windows: gitBashPathToWindowsPath(value),
}));
const suffix = [["", "/w"], ["err\n\n", "/w"], ["a\r\nb\r\n", "C:\\w"]].map(([stderr, root]) => ({
  stderr,
  root,
  result: appendBashCwdStderrSuffix(stderr!, `Shell cwd was reset to ${root}`),
}));
writeFileSync(
  new URL("./shell_snapshot_corpus.json", import.meta.url),
  JSON.stringify({ scripts, captures, gitBash, suffix }, null, 2) + "\n",
);
