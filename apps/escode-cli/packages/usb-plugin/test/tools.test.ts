import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, test } from "vitest";
import { handleUsbToolCall, USB_READ_ONLY_TOOLS, USB_TOOLS, type UsbToolContext } from "../src/tools.js";
import {
  buildUsbmonPcapng,
  bulkExchange,
  ccidDataBlock,
  ccidXfrBlock,
  configWithInterface,
  controlExchange,
  deviceDescriptor,
} from "./fixtures.js";

function context(overrides: Partial<UsbToolContext> = {}): UsbToolContext {
  return {
    env: { PATH: "/nonexistent-path-for-usb-plugin-tests" },
    workspace: "/nonexistent-workspace",
    pluginData: join(tmpdir(), "escode-usb-plugin-test-data"),
    signal: new AbortController().signal,
    ...overrides,
  };
}

function text(result: { content: Array<{ text: string }> }): string {
  return result.content[0]?.text ?? "";
}

/** 与 docs/specs/usb-agent-tools.md 的示例一致的 CCID 抓包：一次失败（6A82）+ 一次成功交换。 */
function ccidCapture(): Uint8Array {
  return buildUsbmonPcapng([
    ...controlExchange({ ts: 1_000, wValue: 0x0100, data: deviceDescriptor(0x072f, 0x223b) }),
    ...controlExchange({ ts: 2_000, wValue: 0x0200, data: configWithInterface(0x0b, 0x00, 0x00) }),
    ...bulkExchange({ ts: 10_000, endpoint: 0x03, data: ccidXfrBlock(7, [0x00, 0xa4, 0x04, 0x00, 0x00]) }),
    ...bulkExchange({ ts: 10_400, endpoint: 0x83, data: ccidDataBlock(7, [0x6a, 0x82]) }),
    ...bulkExchange({ ts: 20_000, endpoint: 0x03, data: ccidXfrBlock(8, [0x00, 0xb0, 0x00, 0x00, 0x04]) }),
    ...bulkExchange({ ts: 20_400, endpoint: 0x83, data: ccidDataBlock(8, [0x01, 0x02, 0x03, 0x04, 0x90, 0x00]) }),
  ]);
}

describe("USB 工具契约", () => {
  test("工具清单、只读标注与 manifest 声明的 server 名一致", async () => {
    assert.deepEqual(
      USB_TOOLS.map((tool) => tool.name),
      ["usb_probe", "usb_capture", "usb_decode"],
    );
    assert.deepEqual(
      USB_TOOLS.filter((tool) => tool.annotations.readOnlyHint).map((tool) => tool.name),
      [...USB_READ_ONLY_TOOLS],
    );
    // usb_capture 会拉起特权抓包进程并写文件：必须留在审批侧（destructiveHint，plan 模式不放行）。
    assert.equal(
      USB_TOOLS.find((tool) => tool.name === "usb_capture")?.annotations.destructiveHint,
      true,
    );
    for (const tool of USB_TOOLS) {
      assert.equal(tool.inputSchema.type, "object", tool.name);
    }

    // 免审批名单在 bootstrap 的 official-plugin-definitions.ts 里被复述了一遍，
    // 这里钉住插件侧：manifest 名、MCP server 名与工具命名前缀必须一致。
    const manifest = JSON.parse(
      await readFile(new URL("../.escode-plugin/plugin.json", import.meta.url), "utf8"),
    ) as { name: string; skills?: string; mcpServers?: Record<string, unknown> };
    assert.equal(manifest.name, "usb");
    assert.deepEqual(Object.keys(manifest.mcpServers ?? {}), ["usb"]);
    assert.equal(manifest.skills, "skills");
    for (const tool of USB_TOOLS) {
      assert.ok(tool.name.startsWith(`${manifest.name}_`), tool.name);
    }
  });

  test("参数严格校验：未知字段与越界值都返回 invalidInput", async () => {
    const unknownField = await handleUsbToolCall({
      name: "usb_decode",
      args: { source: "a.pcapng", interactive: true },
      context: context(),
    });
    assert.equal(unknownField.isError, true);
    assert.match(text(unknownField), /^\[invalidInput\]/u);

    const outOfRange = await handleUsbToolCall({
      name: "usb_capture",
      args: { durationMs: 600_000 },
      context: context(),
    });
    assert.equal(outOfRange.isError, true);
    assert.match(text(outOfRange), /^\[invalidInput\]/u);

    const unknownTool = await handleUsbToolCall({ name: "usb_erase", args: {}, context: context() });
    assert.match(text(unknownTool), /^\[invalidInput\] Unable to |^\[invalidInput\] Unknown tool/u);
  });

  test("usb_probe 在没有抓包后端时给出不可用原因与修复方式，而不是静默失败", async () => {
    const result = await handleUsbToolCall({ name: "usb_probe", args: {}, context: context() });
    assert.equal(result.isError, undefined);
    assert.match(text(result), /平台：/u);
    assert.match(text(result), /抓包：不可用/u);
    assert.match(text(result), /修复方式：/u);
  });

  test("usb_decode 走完整链路：summary → timeline（游标）→ message", async () => {
    const root = await mkdtemp(join(tmpdir(), "escode-usb-decode-"));
    try {
      await writeFile(join(root, "reader.pcapng"), ccidCapture());
      const shared = context({ workspace: root });

      const summary = await handleUsbToolCall({
        name: "usb_decode",
        args: { source: "reader.pcapng", protocol: "ccid", view: "summary" },
        context: shared,
      });
      assert.equal(summary.isError, undefined);
      assert.match(text(summary), /smart-card-ccid/u);
      assert.match(text(summary), /6A82/u);
      assert.match(text(summary), new RegExp(`文件：${root}`, "u"));
      // 元数据块必须是可解析的 JSON，模型可能直接按字段取值。
      const meta = JSON.parse(summary.content[1]?.text ?? "{}") as Record<string, unknown>;
      assert.equal(meta.view, "summary");
      assert.equal(meta.path, join(root, "reader.pcapng"));

      const firstPage = await handleUsbToolCall({
        name: "usb_decode",
        args: { source: "reader.pcapng", protocol: "ccid", view: "timeline", limit: 1 },
        context: shared,
      });
      assert.match(text(firstPage), /#1 /u);
      assert.match(text(firstPage), /cursor=2/u);

      const secondPage = await handleUsbToolCall({
        name: "usb_decode",
        args: { source: "reader.pcapng", protocol: "ccid", view: "timeline", limit: 1, cursor: "2" },
        context: shared,
      });
      assert.match(text(secondPage), /#2 /u);

      const single = await handleUsbToolCall({
        name: "usb_decode",
        args: { source: "reader.pcapng", protocol: "ccid", view: "message", index: 1 },
        context: shared,
      });
      assert.match(text(single), /正文/u);
    } finally {
      await rm(root, { force: true, recursive: true });
    }
  });

  test("抓包文件不存在时返回 notFound（不把路径问题说成格式问题）", async () => {
    const result = await handleUsbToolCall({
      name: "usb_decode",
      args: { source: "missing.pcapng" },
      context: context(),
    });
    assert.equal(result.isError, true);
    assert.match(text(result), /^\[notFound\]/u);
  });

  test("usb_capture 在没有后端时返回 unavailable 与修复方式", async () => {
    const result = await handleUsbToolCall({
      name: "usb_capture",
      args: { durationMs: 1_000 },
      context: context(),
    });
    assert.equal(result.isError, true);
    assert.match(text(result), /^\[unavailable\]/u);
    assert.match(text(result), /下一步：/u);
  });
});
