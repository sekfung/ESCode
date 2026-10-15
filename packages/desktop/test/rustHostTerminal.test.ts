import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { chmod, mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ITerminalService } from "@escode/services/node";
import { createTerminalService } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-terminal.md §5.1 TS oracle 差分：同一个脚本化 SHELL 分别由真实 TS `createTerminalService()`
// （node-pty）与 Rust Host 的 `terminal` channel（真实 `@escode/client`）驱动，转录与返回形状逐项全等。

const SHELL_SCRIPT = `#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    env) printf 'ENV:%s|%s|%s|%s\\n' "$TERM" "$COLORTERM" "$LANG" "$PWD" ;;
    size) printf 'SIZE:%s\\n' "$(stty size)" ;;
    utf8) printf 'U:%s\\n' "中文é🙂" ;;
    exit*) exit "\${line#exit }" ;;
    *) printf 'got:%s\\n' "$line" ;;
  esac
done
`;

async function until(predicate: () => boolean, label: string): Promise<void> {
  const deadline = Date.now() + 10_000;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timeout waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function settle<T>(run: () => Promise<T>): Promise<unknown> {
  try {
    return { ok: (await run()) ?? "<undefined>" };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

/** 同一份脚本驱动一个 `ITerminalService`，返回可比较的转录。 */
async function drive(service: ITerminalService, cwd: string): Promise<Record<string, unknown>> {
  const out: Record<string, unknown> = {};
  const created = await service.create({ cols: 100, rows: 30, cwd });
  out.created = created;
  const id = created.id;
  let data = "";
  let exitCode: number | undefined;
  service.onDynamicData(id)((chunk) => {
    data += chunk;
  });
  service.onDynamicExit(id)((code) => {
    exitCode = code;
  });

  const send = async (line: string, marker: string) => {
    const before = data.length;
    await service.write({ id, data: `${line}\n` });
    await until(() => data.slice(before).includes(marker), marker);
  };
  await send("ping", "got:ping");
  await send("env", "ENV:");
  await send("utf8", "U:");
  await service.resize({ id, cols: 120, rows: 40 });
  await send("size", "SIZE:");
  await service.write({ id, data: "exit 7\n" });
  await until(() => exitCode !== undefined, "exit");
  out.exitCode = exitCode;
  // 输出里含 pty 回显（\r\n）；不同次运行里 PWD 等已由调用方保证一致。
  out.transcript = data;
  out.writeAfterExit = await settle(() => service.write({ id, data: "x" }));
  out.resizeAfterExit = await settle(() => service.resize({ id, cols: 80, rows: 24 }));
  out.disposeAfterExit = await settle(() => service.dispose({ id }));

  // 第二个终端：未退出时 dispose，不应再收到 exit。
  const second = await service.create({ cols: 80, rows: 24, cwd: join(cwd, "does-not-exist") });
  out.secondId = second.id;
  let secondExit: number | undefined;
  service.onDynamicExit(second.id)((code) => {
    secondExit = code;
  });
  await service.dispose({ id: second.id });
  await sleep(600);
  out.secondExit = secondExit ?? "<none>";
  out.writeAfterDispose = await settle(() => service.write({ id: second.id, data: "x" }));
  out.disposeTwice = await settle(() => service.dispose({ id: second.id }));
  return out;
}

describe("rust host terminal domain", () => {
  it.skipIf(!existsSync(hostBinary) || process.platform === "win32")(
    "matches the TS terminal service driven by the same scripted shell",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-terminal-")));
      const home = join(base, "home");
      const work = join(base, "work");
      await mkdir(work, { recursive: true });
      await mkdir(join(home, ".config", "kitty"), { recursive: true });
      await writeFile(join(home, ".config", "kitty", "kitty.conf"), 'font_family  "Kitty Mono"\n');
      const shell = join(base, "shell.sh");
      await writeFile(shell, SHELL_SCRIPT);
      await chmod(shell, 0o755);

      // 两边共用同一份 env：TS 读测试进程 env，Rust Host 读 spawn 时给定的 env。
      const shared = {
        SHELL: shell,
        LANG: "C",
        COLORTERM: " 24bit ",
        TERM: "dumb",
        CI: "1",
        XDG_CONFIG_HOME: "",
      };
      const saved = { ...process.env };
      Object.assign(process.env, shared, { HOME: home, USERPROFILE: home });
      delete process.env.LC_ALL;
      delete process.env.LC_CTYPE;
      const host = startHost(home, shared);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home });
        const listening = await nextMeaningful(host);
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const rustTerminal = services.terminalService as ITerminalService;

        // 全流程差分（默认 settings：继承系统 profile，kitty 配置命中）。
        const tsTerminal = createTerminalService({
          settingService: { get: async () => ({}) } as never,
        });
        const expected = await drive(tsTerminal, work);
        const actual = await drive(rustTerminal, work);
        expect(actual).toEqual(expected);
        (tsTerminal as unknown as { disposeAll(): void }).disposeAll();

        // profile 三态：系统（kitty）→ 自定义字体（与 TS 全等）→ 不继承（回退）。
        const system = await rustTerminal.create({ cols: 80, rows: 24, cwd: work });
        expect(system.fontFamilySource).toBe("system");
        expect(system.fontFamily.startsWith("Kitty Mono, ui-monospace")).toBe(true);
        await rustTerminal.dispose({ id: system.id });

        await services.settingService.update({ terminalFontFamily: "Custom Mono" } as never);
        const custom = await rustTerminal.create({ cols: 80, rows: 24, cwd: work });
        const tsCustom = await createTerminalService({
          settingService: { get: async () => ({ terminalFontFamily: "Custom Mono" }) } as never,
        }).create({ cols: 80, rows: 24, cwd: work });
        expect({ ...custom, id: "_", shell: "_" }).toEqual({ ...tsCustom, id: "_", shell: "_" });
        await rustTerminal.dispose({ id: custom.id });

        await services.settingService.update({
          terminalFontFamily: "",
          terminalInheritSystemProfile: false,
        } as never);
        const plain = await rustTerminal.create({ cols: 80, rows: 24, cwd: work });
        expect(plain.fontFamilySource).toBe("fallback");
        await rustTerminal.dispose({ id: plain.id });
      } finally {
        await host.stop();
        process.env = saved;
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
