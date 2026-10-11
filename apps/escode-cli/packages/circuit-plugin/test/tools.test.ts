import { describe, expect, it } from "vitest";
import { CIRCUIT_TOOL_NAMES, circuitToolResponseSchemas } from "@escode/shared/circuit";
import { CIRCUIT_TOOLS, handleCircuitToolCall, type CircuitBrokerSend } from "../src/tools.js";
import { captureCircuitBrokerConnection, createCircuitBrokerSend } from "../src/broker-client.js";
import { CIRCUIT_BROKER_SOCKET_ENV, CIRCUIT_BROKER_TOKEN_ENV } from "@escode/shared/circuit";

/**
 * circuit-plugin 契约测试（docs/circuit/README.md §5、§10.2）：
 * 工具清单与 annotations（approval / plan 模式）、缺少 broker 时的 unavailable、
 * 分页游标透传与错误码映射。
 */

const META = {
  "com.escode/request-context": { session_id: "session-a", turn_id: "turn-1", runtime_scope: "main" },
};

const summary = {
  irId: "ir:1",
  revision: 2,
  source: { kind: "kicad_sch" as const, path: "board.kicad_sch", contentHash: "0".repeat(64) },
  stale: false,
  componentCount: 1,
  netCount: 1,
  blockCount: 0,
  lowConfidenceCount: 0,
  diagnostics: { total: 0, error: 0, warning: 0, info: 0 },
};

function call(name: string, args: unknown, send: CircuitBrokerSend) {
  return handleCircuitToolCall({ name, args, meta: META, send, signal: new AbortController().signal });
}

const okSend =
  (result: unknown): CircuitBrokerSend =>
  async () => ({ ok: true, result });

describe("circuit-plugin 工具清单", () => {
  it("注册已接入的七个工具，未注册的 G6 工具不出现", () => {
    const names = CIRCUIT_TOOLS.map((tool) => tool.name).sort();
    expect(names).toEqual([
      "circuit_check",
      "circuit_compare_firmware",
      "circuit_correct",
      "circuit_export",
      "circuit_get",
      "circuit_parse",
      "circuit_query",
    ]);
    expect(names).not.toContain(CIRCUIT_TOOL_NAMES.submitRecognition);
  });

  it("annotations 区分只读与写类工具，inputSchema 可由工具 schema 生成", () => {
    const byName = new Map(CIRCUIT_TOOLS.map((tool) => [tool.name, tool]));
    for (const name of ["circuit_get", "circuit_query", "circuit_check", "circuit_compare_firmware"])
      expect(byName.get(name)?.annotations?.readOnlyHint).toBe(true);
    for (const name of ["circuit_correct", "circuit_export"])
      expect(byName.get(name)?.annotations?.destructiveHint).toBe(true);
    // parse 只重建 IR，不改用户文件：非只读、也非破坏性。
    expect(byName.get("circuit_parse")?.annotations).toEqual({ title: "circuit_parse" });
    for (const tool of CIRCUIT_TOOLS) expect(tool.inputSchema.type).toBe("object");
    // export 的请求 schema 带 irId + workspace 相对路径。
    expect(byName.get("circuit_export")?.inputSchema).toMatchObject({
      properties: { irId: expect.anything(), path: expect.anything() },
      required: expect.arrayContaining(["irId", "path"]),
    });
    // 分析工具：check 只要 irId（+ 可选 cursor），compare_firmware 还要 files。
    expect(byName.get("circuit_check")?.inputSchema).toMatchObject({
      properties: { irId: expect.anything(), cursor: expect.anything() },
      required: ["irId"],
    });
    expect(byName.get("circuit_compare_firmware")?.inputSchema).toMatchObject({
      properties: { irId: expect.anything(), files: expect.anything(), cursor: expect.anything() },
      required: expect.arrayContaining(["irId", "files"]),
    });
  });

  it("query 的判别联合在模型侧展开为扁平对象（by 为 enum，字段与 shared 一致）", () => {
    const query = CIRCUIT_TOOLS.find((tool) => tool.name === "circuit_query")!;
    const properties = query.inputSchema.properties as Record<string, { enum?: unknown }>;
    expect(Object.keys(properties).sort()).toEqual([
      "blockId",
      "blockName",
      "by",
      "componentId",
      "cursor",
      "irId",
      "netId",
      "netName",
      "pin",
      "ref",
    ]);
    expect(properties.by?.enum).toEqual(["ref", "net", "block", "trace"]);
    expect(query.inputSchema.required).toEqual(["irId", "by"]);
    expect(query.inputSchema.oneOf).toBeUndefined();
  });

  it("调用转成 broker 请求，带会话上下文与默认 op", async () => {
    const sent: unknown[] = [];
    const send: CircuitBrokerSend = async (request) => {
      sent.push(request);
      return { ok: true, result: { ok: true, summary } };
    };
    const result = await call("circuit_parse", { path: "board.kicad_sch" }, send);
    expect(sent).toEqual([
      {
        op: "parse",
        args: { path: "board.kicad_sch" },
        sessionId: "session-a",
        turnId: "turn-1",
        runtimeScope: "main",
      },
    ]);
    expect(result.isError).toBeUndefined();
    expect(JSON.parse(result.content[0]!.text)).toEqual({ ok: true, summary });
  });

  it("只读分析工具的调用带 op 与会话上下文，结果原样交回模型", async () => {
    const sent: Array<{ op: string; args: unknown }> = [];
    const send: CircuitBrokerSend = async (request) => {
      sent.push({ op: request.op, args: request.args });
      return request.op === "check"
        ? { ok: true, result: { ok: true, config: { status: "default" }, items: [] } }
        : { ok: true, result: { ok: true, items: [], nextCursor: "opaque-2" } };
    };
    const checked = await call("circuit_check", { irId: "ir:1", cursor: "opaque-1" }, send);
    expect(sent[0]).toEqual({ op: "check", args: { irId: "ir:1", cursor: "opaque-1" } });
    expect(JSON.parse(checked.content[0]!.text)).toEqual({
      ok: true,
      config: { status: "default" },
      items: [],
    });
    expect(checked.isError).toBeUndefined();

    const compared = await call(
      "circuit_compare_firmware",
      { irId: "ir:1", files: ["firmware/board.dts"] },
      send,
    );
    expect(sent[1]).toEqual({
      op: "compareFirmware",
      args: { irId: "ir:1", files: ["firmware/board.dts"] },
    });
    expect(JSON.parse(compared.content[0]!.text)).toEqual({
      ok: true,
      items: [],
      nextCursor: "opaque-2",
    });
  });
});

describe("circuit-plugin 分页与错误码", () => {
  it("cursor 原样透传，nextCursor 原样回到模型可见结果", async () => {
    const sent: Array<{ args: unknown }> = [];
    const send: CircuitBrokerSend = async (request) => {
      sent.push({ args: request.args });
      return {
        ok: true,
        result: { ok: true, by: "ref", items: [], nextCursor: "opaque-1" },
      };
    };
    const result = await call(
      "circuit_query",
      { irId: "ir:1", by: "ref", ref: "R1", cursor: "opaque-0" },
      send,
    );
    expect(sent[0]!.args).toEqual({ irId: "ir:1", by: "ref", ref: "R1", cursor: "opaque-0" });
    expect(JSON.parse(result.content[0]!.text)).toMatchObject({ nextCursor: "opaque-1" });
    expect(circuitToolResponseSchemas.query.safeParse(JSON.parse(result.content[0]!.text)).success).toBe(
      true,
    );
  });

  it("服务失败以信封原样返回：错误码就是 CircuitService 的错误码", async () => {
    const failed = await call(
      "circuit_correct",
      { irId: "ir:1", baseRevision: 1, ops: [{ op: "disconnect", componentId: "r1", pin: "1" }] },
      async () => ({
        ok: false,
        error: { code: "stale_revision", message: "当前 revision 为 3，请重读后再写入" },
      }),
    );
    expect(failed.isError).toBe(true);
    expect(JSON.parse(failed.content[0]!.text)).toEqual({
      ok: false,
      error: { code: "stale_revision", message: "当前 revision 为 3，请重读后再写入" },
    });
  });

  it("broker 级错误（规格外错误码）用 [code] 文本，不伪造信封", async () => {
    const result = await call("circuit_get", { irId: "ir:1" }, async () => ({
      ok: false,
      error: { code: "io", message: "Circuit broker closed the connection" },
    }));
    expect(result.isError).toBe(true);
    expect(result.content[0]!.text).toBe("[io] Circuit broker closed the connection");
  });

  it("Host 返回不合契约的载荷时报 io_error，不透传", async () => {
    const result = await call("circuit_get", { irId: "ir:1" }, okSend({ ok: true, summary: {} }));
    expect(result.isError).toBe(true);
    expect(result.content[0]!.text).toMatch(/^\[io_error\] Unexpected circuit_get payload/u);
  });

  it("参数不合法、缺少会话上下文、未知工具都以文本失败", async () => {
    const send = okSend({ ok: true, summary });
    const invalid = await call("circuit_query", { irId: "ir:1", by: "net" }, send);
    expect(invalid.content[0]!.text).toMatch(/^\[invalidInput\]/u);

    const unauthenticated = await handleCircuitToolCall({
      name: "circuit_get",
      args: { irId: "ir:1" },
      meta: {},
      send,
      signal: new AbortController().signal,
    });
    expect(unauthenticated.content[0]!.text).toMatch(/^\[unavailable\]/u);

    const unknown = await call(CIRCUIT_TOOL_NAMES.submitRecognition, { path: "board.pdf" }, send);
    expect(unknown.content[0]!.text).toMatch(/^\[invalidInput\] Unknown tool/u);
  });

  it("分析工具：分页游标与失败信封同样透传，参数在插件层严格校验", async () => {
    const sent: Array<{ args: unknown }> = [];
    const send: CircuitBrokerSend = async (request) => {
      sent.push({ args: request.args });
      return { ok: true, result: { ok: true, items: [], nextCursor: "opaque-2" } };
    };
    const paged = await call(
      "circuit_compare_firmware",
      { irId: "ir:1", files: ["firmware/board.dts"], cursor: "opaque-1" },
      send,
    );
    expect(sent[0]!.args).toEqual({
      irId: "ir:1",
      files: ["firmware/board.dts"],
      cursor: "opaque-1",
    });
    expect(
      circuitToolResponseSchemas.compareFirmware.safeParse(JSON.parse(paged.content[0]!.text)).success,
    ).toBe(true);

    const failed = await call("circuit_check", { irId: "ir:1" }, async () => ({
      ok: false,
      error: { code: "invalid_ir", message: "cursor 与当前查询不匹配" },
    }));
    expect(failed.isError).toBe(true);
    expect(JSON.parse(failed.content[0]!.text)).toEqual({
      ok: false,
      error: { code: "invalid_ir", message: "cursor 与当前查询不匹配" },
    });

    // files 至少一个由 shared 的请求 schema 管，插件层直接以 invalidInput 拒绝。
    const emptyFiles = await call("circuit_compare_firmware", { irId: "ir:1", files: [] }, send);
    expect(emptyFiles.content[0]!.text).toMatch(/^\[invalidInput\]/u);
  });
});

describe("circuit-plugin 没有 broker 材料（纯 CLI/TUI）", () => {
  it("连接材料缺失时返回 unavailable，并提示去 Desktop", async () => {
    expect(captureCircuitBrokerConnection({})).toBeUndefined();
    expect(
      captureCircuitBrokerConnection({
        [CIRCUIT_BROKER_SOCKET_ENV]: "\\\\pipe\\\\x",
        [CIRCUIT_BROKER_TOKEN_ENV]: "t".repeat(64),
      }),
    ).toEqual({ socketPath: "\\\\pipe\\\\x", token: "t".repeat(64) });

    const send = createCircuitBrokerSend(undefined);
    const result = await call("circuit_parse", { path: "board.kicad_sch" }, send);
    expect(result.isError).toBe(true);
    const envelope = JSON.parse(result.content[0]!.text) as {
      ok: boolean;
      error: { code: string; message: string };
    };
    expect(envelope.ok).toBe(false);
    expect(envelope.error.code).toBe("unavailable");
    expect(envelope.error.message).toContain("请在 ESCode Desktop 中使用");
    expect(circuitToolResponseSchemas.parse.safeParse(envelope).success).toBe(true);
  });
});
