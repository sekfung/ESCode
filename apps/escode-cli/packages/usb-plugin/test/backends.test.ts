import assert from "node:assert/strict";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, test } from "vitest";
import { resolveHelperCandidates } from "../src/backends.js";
import { handleUsbToolCall, type UsbToolContext } from "../src/tools.js";
import {
  buildUsbmonPcapng,
  bulkExchange,
  ccidDataBlock,
  ccidXfrBlock,
  configWithInterface,
  controlExchange,
  deviceDescriptor,
} from "./fixtures.js";

/**
 * 桩抓包器：把整条 builtin 路径（spawn → 参数 → 一行 JSON → 产物文件）在**不需要 root 与 usbmon**
 * 的前提下跑通。真实抓包器在 apps/usb-capture-helper（Rust），它的无特权部分有独立单测。
 *
 * shebang 写绝对路径：测试故意把 PATH 设成不存在的目录（让 dumpcap 探测失败），
 * 用 `#!/usr/bin/env node` 会连带把桩自己也跑不起来。
 */
const STUB_SOURCE = `#!${process.execPath}
// CommonJS：抓包器是**无扩展名**的可执行文件，临时目录里没有 package.json，
// Node 按 CJS 解析；写成 ESM 的 import 会在启动阶段直接崩（曾经踩过）。
const { copyFile, stat, writeFile } = require("node:fs/promises");

const argv = process.argv.slice(2);
const value = (name) => {
  const index = argv.indexOf(name);
  return index >= 0 ? argv[index + 1] : undefined;
};
const mode = process.env.USB_TEST_MODE ?? "ok";

const run = async () => {
  if (argv.includes("--probe")) {
    if (mode === "no-helper-permission") {
      console.log(JSON.stringify({ ok: true, platform: "linux", available: false, usbmonModuleLoaded: true, privilege: "denied", interfaces: [], reason: "没有读 /dev/usbmon0 的权限。", remediation: "运行 pkexec escode-usb-capture --install-privileges" }));
      return;
    }
    console.log(JSON.stringify({ ok: true, platform: "linux", available: true, usbmonModuleLoaded: true, privilege: "ok", interfaces: [{ id: "usbmon1", bus: 1, label: "usbmon1" }] }));
    return;
  }
  const output = value("--output");
  if (mode === "permission") {
    await writeFile(output + ".partial", Buffer.alloc(32));
    console.log(JSON.stringify({ ok: false, error: { code: "permission", message: "打不开 /dev/usbmon0", remediation: "运行 pkexec escode-usb-capture --install-privileges" } }));
    process.exitCode = 2;
    return;
  }
  await copyFile(process.env.USB_TEST_FIXTURE, output);
  const info = await stat(output);
  console.log(JSON.stringify({ ok: true, packets: Number(process.env.USB_TEST_PACKETS ?? "6"), bytes: info.size, dropped: Number(process.env.USB_TEST_DROPPED ?? "0"), skippedIsochronous: 0, stoppedBy: "duration", output, bus: Number(value("--bus")) }));
};

run().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
`;

function ccidCapture(): Uint8Array {
  return buildUsbmonPcapng([
    ...controlExchange({ ts: 1_000, wValue: 0x0100, data: deviceDescriptor(0x072f, 0x223b) }),
    ...controlExchange({ ts: 2_000, wValue: 0x0200, data: configWithInterface(0x0b, 0x00, 0x00) }),
    ...bulkExchange({ ts: 10_000, endpoint: 0x03, data: ccidXfrBlock(7, [0x00, 0xa4, 0x04, 0x00, 0x00]) }),
    ...bulkExchange({ ts: 10_400, endpoint: 0x83, data: ccidDataBlock(7, [0x90, 0x00]) }),
  ]);
}

async function withStub(
  run: (input: { root: string; helper: string; fixture: string; context: UsbToolContext }) => Promise<void>,
): Promise<void> {
  const root = await mkdtemp(join(tmpdir(), "escode-usb-backend-"));
  try {
    const helper = join(root, "escode-usb-capture");
    const fixture = join(root, "fixture.pcapng");
    await writeFile(helper, STUB_SOURCE, "utf8");
    await chmod(helper, 0o755);
    await writeFile(fixture, ccidCapture());
    await run({
      root,
      helper,
      fixture,
      context: {
        env: {
          PATH: "/nonexistent-path-for-usb-plugin-tests",
          USB_PLUGIN_CAPTURE_HELPER_PATH: helper,
          USB_TEST_FIXTURE: fixture,
        },
        workspace: root,
        pluginData: join(root, "plugin-data"),
        signal: new AbortController().signal,
      },
    });
  } finally {
    await rm(root, { force: true, recursive: true });
  }
}

describe("自研抓包器后端（builtin）", () => {
  test("路径解析优先级：env 覆盖 > 插件包 bin 目录 > Electron 资源目录", () => {
    const candidates = resolveHelperCandidates({
      env: { USB_PLUGIN_CAPTURE_HELPER_PATH: "/custom/helper", ESCODE_PLUGIN_ROOT: "/plugins/usb" },
      platform: "linux",
      arch: "x64",
      resourcesPath: "/app/resources",
    });
    assert.deepEqual(candidates, [
      "/custom/helper",
      "/plugins/usb/bin/linux-x64/escode-usb-capture",
      "/app/resources/tools/escode-usb-capture",
    ]);
    // Windows 上の可执行文件后缀与投放目录名都不同，必须按平台切换。
    assert.deepEqual(
      resolveHelperCandidates({
        env: { ESCODE_PLUGIN_ROOT: "C:\\plugins\\usb" },
        platform: "win32",
        arch: "arm64",
        resourcesPath: "C:\\app\\resources",
      }),
      [
        join("C:\\plugins\\usb", "bin", "win32-arm64", "escode-usb-capture.exe"),
        join("C:\\app\\resources", "tools", "escode-usb-capture.exe"),
      ],
    );
  });

  test("usb_probe 报告 builtin 为首选后端，并在没有 dumpcap 时仍给出抓包能力", async () => {
    await withStub(async ({ context }) => {
      const result = await handleUsbToolCall({ name: "usb_probe", args: {}, context });
      assert.equal(result.isError, undefined);
      const text = result.content[0]?.text ?? "";
      assert.match(text, /抓包：可用（builtin/u);
      assert.match(text, /抓包接口：usbmon1/u);
      const meta = JSON.parse(result.content[1]?.text ?? "{}") as Record<string, unknown>;
      assert.equal(meta.captureAvailable, true);
    });
  });

  test("抓包器报告权限不足时，probe 说明原因与修复命令，且不显示为可用", async () => {
    await withStub(async ({ context }) => {
      const result = await handleUsbToolCall({
        name: "usb_probe",
        args: {},
        context: { ...context, env: { ...context.env, USB_TEST_MODE: "no-helper-permission" } },
      });
      const text = result.content[0]?.text ?? "";
      assert.match(text, /抓包：不可用/u);
      assert.match(text, /pkexec escode-usb-capture --install-privileges/u);
    });
  });

  test("usb_capture 走 builtin：产物能被 usb_decode 直接分析（端到端）", async () => {
    await withStub(async ({ context, root }) => {
      const capture = await handleUsbToolCall({
        name: "usb_capture",
        args: { durationMs: 1_000, label: "reader-smoke" },
        context,
      });
      assert.equal(capture.isError, undefined);
      const captureText = capture.content[0]?.text ?? "";
      assert.match(captureText, /已抓包：/u);
      assert.match(captureText, /后端：builtin/u);
      assert.match(captureText, /结束原因：duration/u);

      const outputPath = /已抓包：(\S+)/u.exec(captureText)?.[1] ?? "";
      assert.ok(outputPath.endsWith("-reader-smoke.pcapng"), outputPath);
      assert.ok(existsSync(join(root, "plugin-data", "captures")), "默认输出目录应落在 pluginData/captures");

      const decode = await handleUsbToolCall({
        name: "usb_decode",
        args: { source: outputPath, protocol: "ccid", view: "summary" },
        context,
      });
      assert.equal(decode.isError, undefined);
      assert.match(decode.content[0]?.text ?? "", /smart-card-ccid/u);
    });
  });

  test("抓包器上报丢弃事件时，结果里必须带出「可能缺报文」的提示", async () => {
    await withStub(async ({ context }) => {
      const result = await handleUsbToolCall({
        name: "usb_capture",
        args: { durationMs: 1_000 },
        context: { ...context, env: { ...context.env, USB_TEST_DROPPED: "3" } },
      });
      assert.match(result.content[0]?.text ?? "", /丢弃 3 个事件/u);
    });
  });

  test("抓包器失败：错误码映射成工具错误，并保留 .partial 供继续分析", async () => {
    await withStub(async ({ context }) => {
      const result = await handleUsbToolCall({
        name: "usb_capture",
        args: { durationMs: 1_000 },
        context: { ...context, env: { ...context.env, USB_TEST_MODE: "permission" } },
      });
      assert.equal(result.isError, true);
      const text = result.content[0]?.text ?? "";
      assert.match(text, /^\[permission\] 打不开/u);
      assert.match(text, /pkexec escode-usb-capture --install-privileges/u);
      assert.match(text, /已保留部分文件/u);
    });
  });

  test("找不到抓包器时：候选路径进 reason，抓包返回 unavailable", async () => {
    const context: UsbToolContext = {
      env: {
        PATH: "/nonexistent-path-for-usb-plugin-tests",
        USB_PLUGIN_CAPTURE_HELPER_PATH: "/nonexistent/escode-usb-capture",
      },
      workspace: "/nonexistent-workspace",
      pluginData: join(tmpdir(), "escode-usb-plugin-test-data"),
      signal: new AbortController().signal,
    };
    const probe = await handleUsbToolCall({ name: "usb_probe", args: {}, context });
    const probeText = probe.content[0]?.text ?? "";
    assert.match(probeText, /没有找到自研抓包器/u);
    assert.match(probeText, /\/nonexistent\/escode-usb-capture/u);

    const capture = await handleUsbToolCall({
      name: "usb_capture",
      args: { durationMs: 1_000 },
      context,
    });
    assert.equal(capture.isError, true);
    assert.match(capture.content[0]?.text ?? "", /^\[unavailable\]/u);
  });
});
