import { once } from "node:events";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createMarketingTouchFixture } from "./marketing-touch-fixture.js";

/**
 * Marketing Touch E2E 的本地 fixture server。
 *
 * 原先由 `coding-plan-upgrade-mock-server.ts` 顺带承载（同一进程同时提供套餐购买与营销端点）；
 * 套餐族删除（2026-10-15）后升级 mock 移除，这里独立保留营销 fixture 的服务端壳，
 * 使 marketing-touch-* 用例继续可用。
 */
export const MARKETING_TOUCH_MOCK_BASE_URL_ENV = "ESCODE_MARKETING_TOUCH_MOCK_BASE_URL";

type MutableEnvironment = Record<string, string | undefined>;

/** 让 App 的 ApiClient 指向本地 fixture（营销服务读 endpoint origin 下的 /api/v1/marketing/*）。 */
const ENDPOINT_ENV_KEYS = [
  "ESCODE_BASE_URL",
  "ESCODE_ENDPOINT_ORIGIN",
  "ESCODE_TEST_BASE_URL",
] as const;

export function applyMarketingTouchMockEnv(environment: MutableEnvironment, baseUrl: string) {
  environment[MARKETING_TOUCH_MOCK_BASE_URL_ENV] = baseUrl;
  environment.ESCODE_ENV = "test";
  for (const key of ENDPOINT_ENV_KEYS) {
    environment[key] = baseUrl;
  }
}

export function clearMarketingTouchMockEnv(environment: MutableEnvironment, baseUrl: string) {
  for (const key of ENDPOINT_ENV_KEYS) {
    if (environment[key] === baseUrl) delete environment[key];
  }
  if (environment[MARKETING_TOUCH_MOCK_BASE_URL_ENV] === baseUrl) {
    delete environment[MARKETING_TOUCH_MOCK_BASE_URL_ENV];
  }
}

export interface MarketingTouchMockServer {
  baseUrl: string;
  stop: () => Promise<void>;
}

export function shouldPrepareMarketingTouchMock(specs: readonly string[]): boolean {
  return specs.some((spec) => {
    const normalized = spec.replaceAll("\\", "/");
    return normalized === "./test/e2e/**/*.test.ts" || normalized.includes("marketing-touch-");
  });
}

export async function startMarketingTouchMockServer(): Promise<MarketingTouchMockServer> {
  const marketing = await createMarketingTouchFixture();
  const server = createServer((request, response) => {
    void (async () => {
      if (await marketing.handle(request, response)) return;
      sendJson(response, { code: 404, msg: "not found" }, 404);
    })().catch((error: unknown) => {
      if (!response.headersSent) {
        sendJson(response, { code: 500, msg: formatErrorMessage(error) }, 500);
        return;
      }
      response.destroy(error instanceof Error ? error : new Error(String(error)));
    });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("Marketing Touch E2E mock server did not bind to a TCP port");
  }
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    stop: async () => {
      marketing.stop();
      server.close();
      await once(server, "close");
    },
  };
}

function sendJson(response: ServerResponse, value: unknown, status = 200) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

function formatErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
