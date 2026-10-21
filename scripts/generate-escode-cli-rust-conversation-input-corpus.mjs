// Run with node --import tsx. 以 TS withConversationQuotes 为 oracle，导出对话引用在模型边界的
// 材料语料（docs/specs/rust-v3.15.1-drift.md）；Rust conversation_input 逐条比对。
import { readFile, writeFile } from "node:fs/promises";
import { withConversationQuotes } from "../apps/escode-cli/packages/core/src/runtime/helpers/conversation-quotes.ts";

const quote = { text: 'line "one"', path: "/repo/a.ts", senderName: "Ann", senderId: "u1" };
const cases = [
  { name: "plain", text: "hi" },
  { name: "quotes", text: "hi", quotes: [quote, { text: "two" }] },
  { name: "quotes-empty-text", text: "", quotes: [quote] },
];
const corpus = cases.map((c) => ({
  ...c,
  expected: withConversationQuotes(c.text, c.quotes),
}));
const content = `${JSON.stringify({ cases: corpus }, null, 1)}\n`;
const target = new URL(
  "../apps/escode-cli-rust/crates/domain/tests/fixtures/conversation_input_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8").catch(() => "")) !== content)
    throw new Error("Rust conversation input corpus differs from TS");
} else await writeFile(target, content);
