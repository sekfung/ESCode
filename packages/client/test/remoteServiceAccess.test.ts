import { describe, expect, it, vi } from "vitest";
import type { IChannel, IChannelClient } from "@escode/rpc";
import { ISourceServerManagementService } from "@escode/services";
import { RemoteServiceAccess } from "../src/remoteServiceAccess.js";

const channel: IChannel = {
  call: vi.fn(),
  listen: vi.fn(),
};

describe("RemoteServiceAccess", () => {
  it("公开配置经独立频道 RPC 读取", async () => {
    const call = vi.fn(async () => ({ pluginStoreOrder: null }));
    const requested: string[] = [];
    const services = new RemoteServiceAccess({
      getChannel: (name) => {
        requested.push(name);
        return { call, listen: vi.fn() } as IChannel;
      },
    });
    expect(await services.clientConfigService.getSnapshot({ forceRefresh: true })).toEqual({
      pluginStoreOrder: null,
    });
    expect(requested).toContain("client-config");
    expect(call.mock.calls[0]?.slice(0, 2)).toEqual(["getSnapshot", [{ forceRefresh: true }]]);
  });
});

describe("RemoteServiceAccess optional channels", () => {
  it("binds Source management only when the host advertises the channel", async () => {
    const call = vi.fn(async () => ({
      generation: 1,
      profiles: [],
      servers: [],
      workspaceHistory: [],
      states: [],
      failedPathCount: 0,
      abandonedPathCount: 0,
      migrationCounts: [],
    }));
    const getChannel = vi.fn(() => ({ call, listen: vi.fn() }));
    const services = new RemoteServiceAccess({
      getChannel,
      hasChannel: (name: string) => name === ISourceServerManagementService.channelName,
    } as unknown as IChannelClient);

    const service = services.sourceServerManagementService;
    expect(service).toBeDefined();
    await service!.getSnapshot();
    expect(call).toHaveBeenCalledWith("getSnapshot", []);
  });

  it("does not construct a Source management proxy for scopes that do not publish it", () => {
    const getChannel = vi.fn(() => channel);
    const services = new RemoteServiceAccess({
      getChannel,
      hasChannel: () => false,
    } as unknown as IChannelClient);

    expect(services.sourceServerManagementService).toBeUndefined();
    expect(getChannel).not.toHaveBeenCalledWith(ISourceServerManagementService.channelName);
  });
});

describe("RemoteServiceAccess project channels", () => {
  // 回归：Rust Host 已注册 project / task-groups / remote-projects，但 renderer accessor
  // 曾漏绑这三个代理，导致远端目录选择器静默拿到 undefined 而不渲染。
  it("binds project, task-groups and remote-projects channels", async () => {
    const calls: Array<[string, unknown[]]> = [];
    const services = new RemoteServiceAccess({
      getChannel: (name: string) =>
        ({
          call: async (method: string, args: unknown[]) => {
            calls.push([name, [method, ...args]]);
            return {};
          },
          listen: vi.fn(),
        }) as IChannel,
    });

    await services.projectService.readDirectory({ path: "/repo" });
    await services.taskGroupService.read();
    await services.remoteProjectService.readDirectory();

    expect(calls).toEqual([
      ["project", ["readDirectory", { path: "/repo" }]],
      ["task-groups", ["read"]],
      ["remote-projects", ["readDirectory"]],
    ]);
  });
});
