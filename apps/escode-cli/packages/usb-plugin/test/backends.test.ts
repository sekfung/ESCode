import assert from "node:assert/strict";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, test } from "vitest";
import { resolveCaptureBackends, resolveHelperCandidates } from "../src/backends.js";
import { offlineOnlyGuidance } from "../src/probe.js";
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
  if (mode === "signal") {
    await copyFile(process.env.USB_TEST_FIXTURE, output + ".partial");
    console.log(JSON.stringify({ ok: true, packets: 6, bytes: 0, dropped: 0, skippedIsochronous: 0, stoppedBy: "signal", output: output + ".partial", bus: Number(value("--bus")) }));
    return;
  }
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

  test("抓包器被信号中断：按自报的 .partial 收尾，并提示可先分析部分数据", async () => {
    await withStub(async ({ context }) => {
      const result = await handleUsbToolCall({
        name: "usb_capture",
        args: { durationMs: 1_000 },
        context: { ...context, env: { ...context.env, USB_TEST_MODE: "signal" } },
      });
      assert.equal(result.isError, undefined);
      const text = result.content[0]?.text ?? "";
      assert.match(text, /部分文件/u);
      const path = /已抓包：(\S+)/u.exec(text)?.[1] ?? "";
      assert.ok(path.endsWith(".partial"), path);
      const meta = JSON.parse(result.content[1]?.text ?? "{}") as Record<string, unknown>;
      assert.equal(meta.stoppedBy, "signal");
      // 部分文件必须真的能被解码器读（截断容忍），否则「可先分析部分数据」就是空话。
      const decode = await handleUsbToolCall({
        name: "usb_decode",
        args: { source: path, protocol: "ccid", view: "summary" },
        context,
      });
      assert.equal(decode.isError, undefined);
      assert.match(decode.content[0]?.text ?? "", /smart-card-ccid/u);
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

  test("找不到抓包器时：probe 如实报不可用，抓包返回 unavailable（各平台都成立）", async () => {
    const context: UsbToolContext = {
      env: {
        PATH: "/nonexistent-path-for-usb-plugin-tests",
        USB_PLUGIN_CAPTURE_HELPER_PATH: "/nonexistent/escode-usb-capture",
      },
      workspace: "/nonexistent-workspace",
      pluginData: join(tmpdir(), "escode-usb-plugin-test-data"),
      signal: new AbortController().signal,
    };
    // 这里只断言跨平台都成立的部分：CI 会在 macOS / Windows / Linux 上跑，
    // 平台特有的文案（Windows 引导重装、Linux/macOS 的离线指引）在下面的按平台用例里钉。
    const probe = await handleUsbToolCall({ name: "usb_probe", args: {}, context });
    assert.equal(probe.isError, undefined);
    assert.match(probe.content[0]?.text ?? "", /抓包：不可用/u);

    const capture = await handleUsbToolCall({
      name: "usb_capture",
      args: { durationMs: 1_000 },
      context,
    });
    assert.equal(capture.isError, true);
    assert.match(capture.content[0]?.text ?? "", /^\[unavailable\]/u);
  });
});

describe("按平台的后端体面", () => {
  const env = { PATH: "/nonexistent-path-for-usb-plugin-tests" };

  test("Windows：内置抓包服务是唯一后端，缺失时引导重装而不是让用户去装第三方工具", async () => {
    const backends = await resolveCaptureBackends({ env, platform: "win32" });
    assert.equal(backends.backends.length, 1);
    assert.equal(backends.backends[0]?.id, "builtin");
    assert.equal(backends.backends[0]?.available, false);
    assert.match(backends.backends[0]?.reason ?? "", /escode-usb-capture 服务/u);
    assert.match(backends.backends[0]?.remediation ?? "", /重装/u);
    // 明确不要再提「装个 Wireshark/dumpcap」——那条隐性依赖已经删掉。
    assert.doesNotMatch(backends.backends[0]?.remediation ?? "", /dumpcap|Wireshark/u);
    assert.equal(backends.preferred, undefined);
  });

  test("Linux / macOS：不列实时抓包后端，也不给「重装就能抓」的暗示", async () => {
    for (const platform of ["linux", "darwin"] as const) {
      const backends = await resolveCaptureBackends({ env, platform });
      assert.deepEqual(backends.backends, [], platform);
      assert.equal(backends.preferred, undefined, platform);
    }
  });

  test("Linux：只有抓包器确实存在时才登记这个后端（自建场景），不承诺安装", async () => {
    await withStub(async ({ helper }) => {
      const backends = await resolveCaptureBackends({
        env: { ...env, USB_PLUGIN_CAPTURE_HELPER_PATH: helper },
        platform: "linux",
      });
      assert.equal(backends.backends.length, 1);
      assert.equal(backends.backends[0]?.available, true);
      assert.equal(backends.preferred, "builtin");
    });
  });
});

describe("离线指引文案", () => {
  test("Linux 与 macOS 各自说明原因，且都指向「拿别处抓的文件」这条路径", () => {
    const linux = offlineOnlyGuidance("linux");
    assert.match(linux.reason, /实时抓包只在 Windows/u);
    assert.match(linux.remediation, /\.pcapng/u);
    const mac = offlineOnlyGuidance("darwin");
    assert.match(mac.reason, /macOS 没有公开的内核级 USB 抓包接口/u);
    assert.match(mac.remediation, /离线解码/u);
  });
});
