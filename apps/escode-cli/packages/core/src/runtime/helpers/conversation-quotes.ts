import type { TurnInputIntentMetadata } from "@escode/contracts";

/** 引用只在模型边界转为材料，不能混入 canonical 用户正文或解析成控制命令。 */
export function withConversationQuotes(
  text: string,
  quotes: TurnInputIntentMetadata["conversationQuotes"],
): string {
  const parts = [text];
  if (quotes?.length)
    parts.push(`<conversation_quote>\n${JSON.stringify(quotes)}\n</conversation_quote>`);
  return parts.filter(Boolean).join("\n\n");
}
