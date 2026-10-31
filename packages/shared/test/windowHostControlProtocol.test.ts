import { describe, expect, it } from "vitest";
import { HostMessageTypes } from "../src/channels.js";
import { hostIncomingMessageSchema, windowHostAttachmentScopeSchema } from "../src/validation.js";

describe("R1 window Host control protocol", () => {
  it("validates monotonic-shape CUA PiP focus facts", () => {
    expect(
      hostIncomingMessageSchema.safeParse({
        type: "cua-pip-focus-changed",
        event: {
          kind: "focus-changed",
          revision: 3,
          sourceWindowId: "window-7",
          sessionId: "session-a",
        },
      }).success,
    ).toBe(true);
    expect(
      hostIncomingMessageSchema.safeParse({
        type: "cua-pip-focus-changed",
        event: {
          kind: "focus-changed",
          revision: -1,
          sourceWindowId: "window-7",
          sessionId: null,
        },
      }).success,
    ).toBe(false);
  });

  it("remote attachment scope 强制携带完整 session、path 和 identity", () => {
    expect(windowHostAttachmentScopeSchema.safeParse({ kind: "local" }).success).toBe(true);
    expect(
      windowHostAttachmentScopeSchema.safeParse({
        kind: "remote",
        remoteSessionId: "remote-session-1",
        workspacePath: "/work/demo",
        workspaceIdentity: "remote:ssh:dev:/work/demo",
      }).success,
    ).toBe(true);
    expect(
      windowHostAttachmentScopeSchema.safeParse({
        kind: "remote",
        remoteSessionId: "remote-session-1",
        workspacePath: "/work/demo",
      }).success,
    ).toBe(false);
    expect(
      windowHostAttachmentScopeSchema.safeParse({
        kind: "source-settings",
        sourceServerId: "source-1",
      }).success,
    ).toBe(true);
    expect(windowHostAttachmentScopeSchema.safeParse({ kind: "source-settings" }).success).toBe(
      false,
    );
  });

  it("AttachServicePort 同时校验可信 clientMode 和 scope", () => {
    expect(
      hostIncomingMessageSchema.safeParse({
        type: HostMessageTypes.AttachServicePort,
        requestId: "attach-local-1",
        attachmentId: "attachment-local-1",
        clientMode: "desktop-continuous",
        scope: { kind: "local" },
      }).success,
    ).toBe(true);
    expect(
      hostIncomingMessageSchema.safeParse({
        type: HostMessageTypes.AttachServicePort,
        requestId: "attach-remote-1",
        attachmentId: "attachment-remote-1",
        clientMode: "web-remote-replayable",
        scope: {
          kind: "remote",
          remoteSessionId: "remote-session-1",
          workspacePath: "/work/demo",
          workspaceIdentity: "remote:ssh:dev:/work/demo",
        },
      }).success,
    ).toBe(true);
    expect(
      hostIncomingMessageSchema.safeParse({
        type: HostMessageTypes.AttachServicePort,
        requestId: "attach-forged-1",
        attachmentId: "attachment-forged-1",
        clientMode: "mobile-self-declared",
        scope: { kind: "local" },
      }).success,
    ).toBe(false);
  });
});
