import { describe, expect, it, vi } from "vitest";
import { createCuaPermissionMainHandler } from "../src/main/desktopCuaPermissionService.js";

function fakeService() {
  return {
    getStatus: vi.fn(async () => ({ available: false as const, reason: "idle", idle: true })),
    restartHelper: vi.fn(async () => ({ ok: true })),
  };
}

describe("cua-permission Main bridge", () => {
  it("ignores unknown methods so the caller can use its own fallback", () => {
    const service = fakeService();
    const handler = createCuaPermissionMainHandler(() => service as never);
    expect(handler("gen-ui.prepareSandbox", {})).toBeUndefined();
    expect(service.getStatus).not.toHaveBeenCalled();
    expect(service.restartHelper).not.toHaveBeenCalled();
  });

  it("requires workspacePath for getStatus", async () => {
    const service = fakeService();
    const handler = createCuaPermissionMainHandler(() => service as never);
    await expect(handler("cua-permission.getStatus", {})).rejects.toThrow("requires workspacePath");
    expect(service.getStatus).not.toHaveBeenCalled();
  });

  it("forwards optional identity/options verbatim", async () => {
    const service = fakeService();
    const handler = createCuaPermissionMainHandler(() => service as never);
    const options = { includeFunctionalProbes: true };
    await handler("cua-permission.getStatus", {
      workspacePath: "/w",
      workspaceIdentity: "id",
      options,
    });
    expect(service.getStatus).toHaveBeenCalledWith("/w", "id", options);
  });

  it("omits absent fields and supports restartHelper without a workspace", async () => {
    const service = fakeService();
    const handler = createCuaPermissionMainHandler(() => service as never);
    await handler("cua-permission.restartHelper", {
      options: { reason: "permission_granted" },
    });
    expect(service.restartHelper).toHaveBeenCalledWith(undefined, undefined, {
      reason: "permission_granted",
    });
  });

  it("rejects malformed parameters", async () => {
    const service = fakeService();
    const handler = createCuaPermissionMainHandler(() => service as never);
    await expect(handler("cua-permission.getStatus", { workspacePath: 1 })).rejects.toThrow(
      "workspacePath must be a string",
    );
    await expect(handler("cua-permission.restartHelper", { options: [] })).rejects.toThrow(
      "options must be an object",
    );
  });
});
