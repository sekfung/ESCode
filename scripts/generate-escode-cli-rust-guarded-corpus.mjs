// Run with node --import tsx. 以 TS matchDangerousCommand 为 oracle，导出 guarded 危险命令判定语料
// （docs/specs/rust-v3.15.1-drift.md）。命令来自上游 guarded 用例里的全部字符串字面量（不手抄），
// 再补几条 Rust 解析器边界用例；每条按 posix / git-bash / cmd 三种方言各判一次。
import { readFile, writeFile } from "node:fs/promises";
import ts from "typescript";
import { matchDangerousCommand } from "../apps/escode-cli/packages/core/src/tool/handlers/guarded/command.ts";

const sources = [
  "../apps/escode-cli/packages/core/tests/guarded-command.test.ts",
  "../apps/escode-cli/packages/core/tests/guarded-deletion.test.ts",
];
const extra = [
  "time rm -rf build",
  "time -p rm -rf build",
  "time -o out.txt rm -rf build",
  "sudo -u root env FOO=1 rm -rf /tmp/x",
  "env -i PATH=/bin rm -r a",
  "echo ok && git reset --hard",
  "ls | xargs rm -rf",
  "rm a b",
  "rm *.log",
  "rm 'a*'",
  "rm $(echo *)",
  "rm -rf ~/cache",
  "find . -name '*.tmp' -delete",
  "find . -exec rm {} ;",
  "git push origin main",
  "git stash list",
  "robocopy src dst /MIR",
  "robocopy src dst /MIR /L",
  "rd /s /q build",
  "del /q *.tmp",
  "C:\\Windows\\System32\\cmd.exe /c rd /s /q x",
  "RM.EXE -rf x",
  "",
  "   ",
];

const strings = new Set();
for (const relative of sources) {
  const path = new URL(relative, import.meta.url);
  const text = await readFile(path, "utf8");
  const file = ts.createSourceFile(path.pathname, text, ts.ScriptTarget.Latest, true);
  const visit = (node) => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      const value = node.text;
      // 模块路径与超长文本不是命令语料。
      if (value.length <= 400 && !value.startsWith("../") && !value.startsWith("node:"))
        strings.add(value);
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
}
for (const command of extra) strings.add(command);

const cases = [...strings]
  .sort()
  .flatMap((command) =>
    ["posix", "git-bash", "cmd"].map((dialect) => ({
      command,
      dialect,
      result: matchDangerousCommand(command, dialect),
    })),
  );
const content = `${JSON.stringify({ cases }, null, 1)}\n`;
const target = new URL(
  "../apps/escode-cli-rust/crates/domain/tests/fixtures/guarded_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  const current = await readFile(target, "utf8").catch(() => "");
  if (current !== content) throw new Error("Rust guarded corpus differs from TS");
} else await writeFile(target, content);
