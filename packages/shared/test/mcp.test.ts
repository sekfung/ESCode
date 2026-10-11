import { describe, expect, it } from "vitest";
import {
  convertToESCodeAgentMcpServer,
  isESCodeCuaMcpCommand,
  isESCodeCuaMcpPackageArg,
} from "../src/mcp.js";

describe("escode-cua MCP matcher (single source of truth for services + CLI bootstrap)", () => {
  it("matches the escode-cua command by leaf name", () => {
    expect(isESCodeCuaMcpCommand("escode-cua")).toBe(true);
    expect(isESCodeCuaMcpCommand("/opt/homebrew/bin/escode-cua")).toBe(true);
    expect(isESCodeCuaMcpCommand("uvx")).toBe(false);
    expect(isESCodeCuaMcpCommand("escode-cua-server")).toBe(false);
  });

  it("matches uvx/pip package spec forms", () => {
    for (const arg of ["escode-cua", "escode-cua[macos]", "escode-cua@v0.1.3", "escode-cua[macos]@v0.1.3"]) {
      expect(isESCodeCuaMcpPackageArg(arg)).toBe(true);
    }
  });

  it("matches git and local-path forms (the drift that broke desktop/services parity)", () => {
    expect(isESCodeCuaMcpPackageArg("escode-cua.git")).toBe(true);
    expect(isESCodeCuaMcpPackageArg("git+https://github.com/x/escode-cua.git@v0.1.3")).toBe(true);
    expect(isESCodeCuaMcpPackageArg("/Users/me/src/escode-cua")).toBe(true);
    expect(isESCodeCuaMcpPackageArg("C:\\src\\escode-cua")).toBe(true);
  });

  it("does not match unrelated args", () => {
    for (const arg of ["--from", "run", "escode-cua-server", "not-escode-cua", "server.js"]) {
      expect(isESCodeCuaMcpPackageArg(arg)).toBe(false);
    }
  });

  it("recognizes a package spec placed in COMMAND position (command/arg matchers stay consistent)", () => {
    // 之前 command matcher 只认精确叶子 escode-cua，会漏判把包规格直接当 command 的配置 → 漏注入 broker。
    for (const command of ["escode-cua@1.2.3", "escode-cua[macos]", "escode-cua.git", "/opt/tools/escode-cua@1.2.3"]) {
      expect(isESCodeCuaMcpCommand(command), command).toBe(true);
    }
  });

  it("recognizes pip == pins and the escode_cua underscore distribution name (PyPI treats _ == -)", () => {
    for (const arg of ["escode-cua==1.2.3", "escode_cua", "escode_cua==1.2.3", "escode_cua@v0.1.3"]) {
      expect(isESCodeCuaMcpPackageArg(arg), arg).toBe(true);
    }
    expect(isESCodeCuaMcpCommand("escode_cua")).toBe(true);
  });

  it("does not false-positive on sibling packages like escode-cua-proxy", () => {
    for (const value of ["escode-cua-proxy", "escode-cua-proxy@1.0.0", "escode-cua-helper"]) {
      expect(isESCodeCuaMcpCommand(value), value).toBe(false);
      expect(isESCodeCuaMcpPackageArg(value), value).toBe(false);
    }
  });

  it("recognizes `python -m escode_cua.server` submodule form (M1 fail-open fix)", () => {
    // `python -m escode_cua.server` → arg 是 `escode_cua.server`；旧 matcher 漏判 → 不注入 broker → fail-open。
    for (const arg of ["escode_cua.server", "escode-cua.server", "escode_cua.main", "escode-cua.__main__"]) {
      expect(isESCodeCuaMcpPackageArg(arg), arg).toBe(true);
    }
  });

  it("recognizes local paths with a trailing separator (M1 fail-open fix)", () => {
    // `.../escode-cua/` 直接取叶子会得到空串 → 漏判 → fail-open。
    expect(isESCodeCuaMcpPackageArg("/Users/me/src/escode-cua/")).toBe(true);
    expect(isESCodeCuaMcpPackageArg("C:\\src\\escode-cua\\")).toBe(true);
    expect(isESCodeCuaMcpCommand("/opt/homebrew/bin/escode-cua/")).toBe(true);
    // 但带尾斜杠的 sibling 仍不误判。
    expect(isESCodeCuaMcpPackageArg("/Users/me/src/escode-cua-proxy/")).toBe(false);
  });
});

describe("MCP config helpers", () => {
  it("passes legacy http_headers through to ESCode Agent MCP servers", () => {
    expect(
      convertToESCodeAgentMcpServer("web-search-prime", {
        type: "http",
        url: "https://bigmodel.example.test/mcp",
        timeoutMs: 5000,
        http_headers: {
          Authorization: "Bearer token",
        },
      }),
    ).toEqual({
      name: "web-search-prime",
      type: "http",
      url: "https://bigmodel.example.test/mcp",
      headers: [{ name: "Authorization", value: "Bearer token" }],
      timeoutMs: 5000,
    });
  });

  it("preserves timeoutMs when converting stdio MCP servers for ESCode Agent", () => {
    expect(
      convertToESCodeAgentMcpServer("context7", {
        type: "stdio",
        command: "npx",
        args: ["-y", "@upstash/context7-mcp"],
        timeoutMs: 3,
      }),
    ).toEqual({
      name: "context7",
      command: "npx",
      args: ["-y", "@upstash/context7-mcp"],
      env: [],
      timeoutMs: 3,
    });
  });

  it("preserves timeoutMs when converting sse MCP servers for ESCode Agent", () => {
    expect(
      convertToESCodeAgentMcpServer("stream", {
        type: "sse",
        url: "https://bigmodel.example.test/sse",
        timeoutMs: 8000,
      }),
    ).toEqual({
      name: "stream",
      type: "sse",
      url: "https://bigmodel.example.test/sse",
      headers: [],
      timeoutMs: 8000,
    });
  });

  it("preserves HTTP OAuth client credentials when converting MCP servers for ESCode Agent", () => {
    expect(
      convertToESCodeAgentMcpServer("protected", {
        type: "http",
        url: "https://mcp.example.test/mcp",
        oauth: {
          type: "client_credentials",
          clientId: "escode-client",
          clientSecret: "secret",
          scope: "mcp:tools",
        },
      }),
    ).toMatchObject({
      name: "protected",
      type: "http",
      oauth: {
        type: "client_credentials",
        clientId: "escode-client",
        clientSecret: "secret",
        scope: "mcp:tools",
      },
    });
  });

  it("preserves HTTP OAuth authorization code config when converting MCP servers for ESCode Agent", () => {
    expect(
      convertToESCodeAgentMcpServer("figma", {
        type: "http",
        url: "https://mcp.figma.com/mcp",
        oauth: {
          type: "authorization_code",
          clientName: "ESCode",
          redirectPath: "/oauth/callback/mcp/figma",
          scope: "mcp:connect",
        },
      }),
    ).toMatchObject({
      name: "figma",
      type: "http",
      oauth: {
        type: "authorization_code",
        clientName: "ESCode",
        redirectPath: "/oauth/callback/mcp/figma",
        scope: "mcp:connect",
      },
    });
  });

  it("does not persist implicit OAuth defaults when converting bare HTTP MCP servers", () => {
    expect(
      convertToESCodeAgentMcpServer("notion", {
        type: "http",
        url: "https://mcp.notion.com/mcp",
        timeoutMs: 30000,
      }),
    ).toEqual({
      name: "notion",
      type: "http",
      url: "https://mcp.notion.com/mcp",
      headers: [],
      timeoutMs: 30000,
    });
  });

  it("drops invalid timeoutMs when converting MCP servers for ESCode Agent", () => {
    expect(
      convertToESCodeAgentMcpServer("zero", {
        type: "stdio",
        command: "npx",
        timeoutMs: 0,
      }),
    ).not.toHaveProperty("timeoutMs");

    expect(
      convertToESCodeAgentMcpServer("fractional", {
        type: "http",
        url: "https://bigmodel.example.test/mcp",
        timeoutMs: 1.5,
      }),
    ).not.toHaveProperty("timeoutMs");
  });

  it("keeps headers ahead of legacy http_headers when both are present", () => {
    expect(
      convertToESCodeAgentMcpServer("web-reader", {
        type: "sse",
        url: "https://bigmodel.example.test/sse",
        headers: {
          Authorization: "Bearer fresh",
        },
        http_headers: {
          Authorization: "Bearer stale",
        },
      }),
    ).toMatchObject({
      headers: [{ name: "Authorization", value: "Bearer fresh" }],
    });
  });
});
