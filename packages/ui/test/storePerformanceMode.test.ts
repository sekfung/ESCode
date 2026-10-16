// 原 storeOAuthRestoreState.test.ts：账号登录链（isRestoringOAuthSession / requestLoginEntry /
// setUser / markOAuthSuccess / coding plan 重置入口）随套餐族与账号登录下线，仅保留与登录无关的
// store 行为断言（性能模式持久化），文件随之改名。
import type { IBroadcastService } from "@escode/services";
import { beforeEach, describe, expect, it } from "vitest";
import { createESCodeStore } from "@/store/index.js";

const localStorageState = new Map<string, string>();

Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: {
    getItem: (key: string) => localStorageState.get(key) ?? null,
    setItem: (key: string, value: string) => {
      localStorageState.set(key, value);
    },
    removeItem: (key: string) => {
      localStorageState.delete(key);
    },
    clear: () => {
      localStorageState.clear();
    },
  },
});

Object.defineProperty(globalThis, "document", {
  configurable: true,
  value: {
    documentElement: {
      classList: {
        contains: () => false,
        toggle: () => {},
      },
    },
  },
});

const mockBroadcastService: IBroadcastService = {
  send: async () => {},
  onMessage: () => ({ dispose: () => {} }),
};

describe("createESCodeStore performance mode", () => {
  beforeEach(() => {
    localStorageState.clear();
  });

  it("性能模式默认关闭，并持久化用户选择", () => {
    const store = createESCodeStore(mockBroadcastService);

    expect(store.getState().performanceMode).toBe(false);

    store.getState().setPerformanceMode(true);

    expect(store.getState().performanceMode).toBe(true);
    expect(localStorageState.get("escode-performance-mode")).toBe("true");
    expect(createESCodeStore(mockBroadcastService).getState().performanceMode).toBe(true);
  });
});
