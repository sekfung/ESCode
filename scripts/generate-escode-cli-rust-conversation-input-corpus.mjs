// Run with node --import tsx. 以 TS withConversationQuotes 为 oracle，导出对话引用与机器人群来源在模型边界的
// 材料语料（docs/specs/rust-v3.15.1-drift.md）；Rust conversation_input 逐条比对。
import { readFile, writeFile } from "node:fs/promises";
import { withConversationQuotes } from "../apps/escode-cli/packages/core/src/runtime/helpers/conversation-quotes.ts";

const quote = { text: 'line "one"', path: "/repo/a.ts", senderName: "Ann", senderId: "u1" };
const mention = (refId, name, targetId) => ({
  type: "channelMention",
  refId,
  name,
  channel: "feishu",
  ...(targetId === undefined ? {} : { targetId }),
});
const baseSource = {
  botId: "bot_1",
  provider: "feishu",
  chatId: "oc_1",
  senderId: "ou_s",
  senderName: "Sender",
  messageId: "om_1",
};
const cases = [
  { name: "plain", text: "hi" },
  { name: "quotes", text: "hi", quotes: [quote, { text: "two" }] },
  { name: "quotes-empty-text", text: "", quotes: [quote] },
  { name: "bot-no-identity", text: "hi", botSource: baseSource },
  {
    name: "bot-identity",
    text: "hi",
    botSource: {
      ...baseSource,
      botIdentity: { name: "Helper", openId: "ou_bot" },
      mentionedBot: true,
    },
  },
  {
    name: "bot-mentions",
    text: "please",
    quotes: [quote],
    botSource: {
      ...baseSource,
      botIdentity: { name: "Helper", openId: "ou_bot" },
      contentParts: [
        { type: "text", text: "hey " },
        mention("r1", "Helper", "ou_bot"),
        { type: "text", text: " and " },
        mention("r2", "Bob", "ou_bob"),
      ],
    },
  },
  {
    name: "bot-topic-messages",
    text: "batch",
    botSource: {
      ...baseSource,
      threadId: "t1",
      messageId: "om_2",
      messages: [
        {
          messageId: "om_1",
          senderId: "ou_a",
          senderName: "A",
          text: "first",
          attachmentIndexes: [],
          contentParts: [mention("r3", "Carol")],
        },
        {
          messageId: "om_2",
          senderId: "ou_b",
          senderName: "B",
          mentionedBot: true,
          text: "second",
          attachmentIndexes: [],
        },
      ],
    },
  },
];
const corpus = cases.map((c) => ({
  ...c,
  expected: withConversationQuotes(c.text, c.quotes, c.botSource),
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
