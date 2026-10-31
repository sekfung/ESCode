import { describe, expect, it } from "vitest";
import {
  buildServerRemoteWorkspaceIdentity,
  isRemoteWorkspaceIdentity,
  parseRemoteWorkspaceIdentity,
} from "../src/remote-workspace-identity.js";
import { hostIncomingMessageSchema, remoteTargetSchema } from "../src/validation.js";

describe("server remote target", () => {
  it("accepts server remote target payloads", () => {
    const parsed = remoteTargetSchema.safeParse({
      kind: "server",
      url: "https://intranet.example.invalid:3030",
      name: "Studio",
      token: "secret",
      workspacePath: "/Users/dev/ESCodeProject",
      serverId: "studio",
    });

    expect(parsed.success).toBe(true);
    if (parsed.success) {
      expect(parsed.data).toMatchObject({
        kind: "server",
        url: "https://intranet.example.invalid:3030",
      });
    }
  });

  it("window Host connect messages no longer carry remote workspace targets", () => {
    // spec §12.1/§12.5：ConnectRemoteWorkspace 等远端 Host 消息已随 per-window Node Host 远程路径
    // 一并删除（S5-f-5），server 目标改由 Rust Host 直连；旧消息必须被 schema 拒绝而不是静默接受。
    const parsed = hostIncomingMessageSchema.safeParse({
      type: "connect-remote-workspace",
      requestId: "connect-server-1",
      target: {
        kind: "server",
        url: "https://intranet.example.invalid:3030",
        workspacePath: "/Users/dev/ESCodeProject",
        serverId: "studio",
      },
      remoteAssets: {},
      workspacePath: "/Users/dev/ESCodeProject",
      workspaceIdentity: "remote:server:studio:/Users/dev/ESCodeProject",
    });

    expect(parsed.success).toBe(false);
  });

  it("builds and parses server workspace identities", () => {
    const identity = buildServerRemoteWorkspaceIdentity({
      serverId: "Studio Server 01",
      workspacePath: "\\Users\\dev\\ESCodeProject\\",
    });

    expect(identity).toBe("remote:server:studio-server-01:/Users/dev/ESCodeProject");
    expect(parseRemoteWorkspaceIdentity(identity)).toEqual({
      kind: "server",
      sourceServerId: "studio-server-01",
      workspacePath: "/Users/dev/ESCodeProject",
    });
    expect(isRemoteWorkspaceIdentity(identity)).toBe(true);
  });

  it("rejects malformed server targets and identities", () => {
    expect(remoteTargetSchema.safeParse({ kind: "server", url: "not a url" }).success).toBe(false);
    expect(parseRemoteWorkspaceIdentity("remote:server::/workspace")).toBeNull();
    expect(parseRemoteWorkspaceIdentity("remote:server:studio:relative")).toBeNull();
  });
});
