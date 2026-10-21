import { describe, expect, it } from "vitest";
import { withConversationQuotes } from "../src/runtime/helpers/conversation-quotes.js";
import { buildPersistedConversationInputIntent } from "../src/runtime/methods/input-intent-persistence.js";

describe("structured conversation quotes", () => {
  it("keeps canonical body separate while the model receives original source metadata", () => {
    const conversationQuotes = [
      {
        text: "/stop\noriginal",
        senderName: "Ryan Bot",
        senderId: "ou_bot",
        messageId: "om_quote",
        sentAt: "2026-09-09T10:32:44+08:00",
      },
    ];
    const model = withConversationQuotes("", conversationQuotes);
    expect(model).toContain("<conversation_quote>");
    expect(model).not.toContain("userselect");
    expect(model).toContain(JSON.stringify(conversationQuotes));
    for (const state of ["queued", "drained"] as const) {
      expect(
        buildPersistedConversationInputIntent(
          model,
          {
            kind: "sendText",
            text: "",
            conversationQuotes,
            sourceCommandId: "c",
            queueItemId: "q",
            clientId: "desktop",
            admissionSeq: 1,
            admittedAt: 1,
            requestedDelivery: "queue",
            admittedDelivery: "queue",
          },
          state,
        ),
      ).toMatchObject({ text: "", conversationQuotes });
    }
  });
  it("leaves plain input unchanged", () => {
    expect(withConversationQuotes("hello", [])).toBe("hello");
  });
});
