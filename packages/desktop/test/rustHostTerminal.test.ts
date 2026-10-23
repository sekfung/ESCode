import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// Phase 3 golden freeze：TS `createTerminalService()`（node-pty）的观察已冻结为仓内资产
// （packages/desktop/test/fixtures/golden/rustHostTerminal.oracle.json）。测试不再实例化 TS 服务，
// 删除 TS 实现后仍能独立重验 Rust Host 的 `terminal` channel（真实 `@escode/client`）。
const goldenFile = new URL("./fixtures/golden/rustHostTerminal.oracle.json", import.meta.url);

// docs/specs/rust-u2-terminal.md §5.1 差分：同一个脚本化 SHELL 由 Rust Host 的 `terminal` channel
// （真实 `@escode/client`）驱动，转录与返回形状对冻结的 TS 观察逐项全等。

type TerminalService = Awaited<ReturnType<typeof connectRpc>>["terminalService"];

/** 与生成脚本一致：绝对临时路径归一化为 `$FIXTURE_ROOT`，终端 id 归一化为固定 token。 */
function normalize(value: unknown, root: string, ids: [string, string]): unknown {
  const [first, second] = ids;
  let text = JSON.stringify(value).split(root).join("$FIXTURE_ROOT");
  text = text
    .split(`Terminal not found: ${first}`)
    .join("Terminal not found: $TERMINAL_ID")
    .split(`Terminal not found: ${second}`)
    .join("Terminal not found: $SECOND_TERMINAL_ID")
    .split(`"${first}"`)
    .join('"$TERMINAL_ID"')
    .split(`"${second}"`)
    .join('"$SECOND_TERMINAL_ID"');
  return JSON.parse(text);
}

/** profile 三态只比较字体/来源等创建结果字段（id/shell 不计入冻结观察）。 */
function profileShape(created: Record<string, unknown>): Record<string, unknown> {
  const { id: _id, shell: _shell, ...rest } = created;
  return rest;
}

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

/** 同一份脚本驱动终端服务，返回可比较的转录（与生成脚本完全一致）。 */
async function drive(service: TerminalService, cwd: string): Promise<Record<string, unknown>> {
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

      // Rust Host 读 spawn 时给定的 env；生成脚本录制 TS 观察时使用同一组值。
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
        const rustTerminal = services.terminalService;

        const golden = JSON.parse(await readFile(goldenFile, "utf8")) as {
          drive: Record<string, unknown>;
          profiles: {
            system: Record<string, unknown>;
            custom: Record<string, unknown>;
            plain: Record<string, unknown>;
          };
        };

        // 全流程差分（默认 settings：继承系统 profile，kitty 配置命中）。
        const actual = await drive(rustTerminal, work);
        const ids: [string, string] = [
          String((actual.created as { id: string }).id),
          String(actual.secondId),
        ];
        expect(normalize(actual, base, ids)).toEqual(golden.drive);

        // profile 三态：系统（kitty）→ 自定义字体 → 不继承（回退）。
        const system = await rustTerminal.create({ cols: 80, rows: 24, cwd: work });
        expect(profileShape(system as unknown as Record<string, unknown>)).toEqual(
          golden.profiles.system,
        );
        await rustTerminal.dispose({ id: system.id });

        await services.settingService.update({ terminalFontFamily: "Custom Mono" } as never);
        const custom = await rustTerminal.create({ cols: 80, rows: 24, cwd: work });
        expect(profileShape(custom as unknown as Record<string, unknown>)).toEqual(
          golden.profiles.custom,
        );
        await rustTerminal.dispose({ id: custom.id });

        await services.settingService.update({
          terminalFontFamily: "",
          terminalInheritSystemProfile: false,
        } as never);
        const plain = await rustTerminal.create({ cols: 80, rows: 24, cwd: work });
        expect(profileShape(plain as unknown as Record<string, unknown>)).toEqual(
          golden.profiles.plain,
        );
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
