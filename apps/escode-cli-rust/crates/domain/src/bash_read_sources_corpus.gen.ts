// 生成 bash_read_sources_corpus.json 的 TS oracle 脚本（docs/specs/rust-bash-model-content.md）：
// node --import tsx apps/escode-cli-rust/crates/domain/src/bash_read_sources_corpus.gen.ts
import { writeFileSync } from "node:fs";
import {
  collectBashReadFileSources,
  selectReadContent,
} from "../../../../escode-cli/packages/core/src/tool/handlers/bash-read-file-sources.ts";

const commands = [
  "cat a.txt",
  "cat -n src/main.rs",
  "cat --number x",
  "cat -A x",
  "cat a b",
  "cat -",
  "head a.txt",
  "head -n 5 a.txt",
  "head -n5 a.txt",
  "head -20 a.txt",
  "head --lines=3 a.txt",
  "head -n 0 a.txt",
  "head -c 10 a.txt",
  "tail a.txt",
  "tail -n 3 log.txt",
  "sed -n '5,10p' a.txt",
  "sed -n 7p a.txt",
  "sed '5,10p' a.txt",
  "sed -n -e 5p a.txt",
  "sed -i 's/a/b/' a.txt",
  "sed --quiet 2p a.txt",
  "grep foo a.txt",
  "grep -n foo a.txt",
  "grep -A 2 foo a.txt",
  "grep -C3 foo a.txt",
  "grep --context=2 foo a.txt",
  "grep -r foo .",
  "grep foo '*.txt'",
  "grep foo a.txt b.txt",
  "echo hi && cat a.txt",
  "cat a.txt; head -n 2 b.txt",
  "echo hi && grep foo a.txt",
  "cd x && cat a.txt",
  "cat a.txt | head",
  "cat a.txt > b.txt",
  "cat $FILE",
  ": && cat a.txt",
  "true; cat a.txt",
  "printf x; tail -n 1 a.txt",
];
const content = "one\ntwo\nthree\nfour\n";
const cases = commands.map((command) => {
  const sources = collectBashReadFileSources(command);
  return {
    command,
    sources,
    selected: sources.map((source) => selectReadContent(content, source)?.content ?? null),
  };
});
writeFileSync(
  new URL("./bash_read_sources_corpus.json", import.meta.url),
  JSON.stringify({ content, cases }, null, 2) + "\n",
);
