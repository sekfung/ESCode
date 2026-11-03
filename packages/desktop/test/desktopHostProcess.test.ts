import { afterEach, describe, expect, it, vi } from "vitest";
import { loadWindow } from "../src/main/desktopHostProcess.js";

const electronMock = vi.hoisted(() => ({
  isPackaged: false,
}));

afterEach(() => {
  vi.unstubAllEnvs();
  electronMock.isPackaged = false;
});

vi.mock("electron", () => ({
  app: {
    getPath: vi.fn(() => "/tmp/escode-test"),
    get isPackaged() {
      return electronMock.isPackaged;
    },
  },
}));

describe("desktopHostProcess loadWindow", () => {
  it("生产包忽略继承的开发服务器地址并加载内置页面", async () => {
    electronMock.isPackaged = true;
    vi.stubEnv("ELECTRON_RENDERER_URL", "http://localhost:5174");
    const window = {
      loadFile: vi.fn().mockResolvedValue(undefined),
      loadURL: vi.fn().mockResolvedValue(undefined),
    };

    await loadWindow(window as never);

    expect(window.loadURL).not.toHaveBeenCalled();
    expect(window.loadFile).toHaveBeenCalledOnce();
  });
});
