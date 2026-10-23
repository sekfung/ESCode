// Phase 3 golden freeze：rustHostTerminal.test.ts 的 TS `createTerminalService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：临时根路径写成 `$FIXTURE_ROOT`，终端 id 写成固定 token；pty 转录本身是确定性的
// （脚本固定、等待每个 marker 后再继续），因此完整保留 raw 转录做逐字节比较。
import { deepEqual } from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createTerminalService } from "../packages/services/src/node.ts";

type TerminalService = ReturnType<typeof createTerminalService>;

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

/** 同一份脚本驱动一个 `ITerminalService`，返回可比较的转录（与测试中的 drive 完全一致）。 */
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

/** 绝对路径与终端 id 的归一化；测试侧对 Rust 输出做同一变换。 */
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

/** profile 三态只比较字体/来源等创建结果字段（id/shell 在测试中同样剔除）。 */
function profileShape(
  created: Awaited<ReturnType<TerminalService["create"]>>,
): Record<string, unknown> {
  const { id: _id, shell: _shell, ...rest } = created;
  return rest;
}

const base = await realpath(await mkdtemp(join(tmpdir(), "rusthost-terminal-golden-")));
const home = join(base, "home");
const work = join(base, "work");
await mkdir(work, { recursive: true });
await mkdir(join(home, ".config", "kitty"), { recursive: true });
await writeFile(join(home, ".config", "kitty", "kitty.conf"), 'font_family  "Kitty Mono"\n');
const shell = join(base, "shell.sh");
await writeFile(shell, SHELL_SCRIPT);
await chmod(shell, 0o755);

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

const goldenDir = "packages/desktop/test/fixtures/golden";
const jsonPath = join(goldenDir, "rustHostTerminal.oracle.json");

try {
  const service = createTerminalService({
    settingService: { get: async () => ({}) } as never,
  });
  const driveObservation = await drive(service, work);
  service.disposeAll();

  const systemService = createTerminalService({
    settingService: { get: async () => ({}) } as never,
  });
  const system = profileShape(await systemService.create({ cols: 80, rows: 24, cwd: work }));
  systemService.disposeAll();

  const customService = createTerminalService({
    settingService: { get: async () => ({ terminalFontFamily: "Custom Mono" }) } as never,
  });
  const custom = profileShape(await customService.create({ cols: 80, rows: 24, cwd: work }));
  customService.disposeAll();

  const plainService = createTerminalService({
    settingService: {
      get: async () => ({
        terminalFontFamily: "",
        terminalInheritSystemProfile: false,
      }),
    } as never,
  });
  const plain = profileShape(await plainService.create({ cols: 80, rows: 24, cwd: work }));
  plainService.disposeAll();

  const ids: [string, string] = [
    String((driveObservation.created as { id: string }).id),
    String(driveObservation.secondId),
  ];
  const golden = {
    _readme:
      "rustHostTerminal.test.ts 的 TS createTerminalService 观察冻结（Phase 3 golden freeze）。绝对临时路径归一化为 $FIXTURE_ROOT，终端 id 归一化为 $TERMINAL_ID / $SECOND_TERMINAL_ID；测试对 Rust Host 输出做同一变换。",
    drive: normalize(driveObservation, base, ids),
    profiles: { system, custom, plain },
  };
  const json = `${JSON.stringify(golden, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    deepEqual(
      JSON.parse(await readFile(jsonPath, "utf8")),
      JSON.parse(json),
      "rustHostTerminal golden 与当前 TS 观察漂移",
    );
    console.log("rustHostTerminal golden: OK");
  } else {
    await mkdir(goldenDir, { recursive: true });
    await writeFile(jsonPath, json);
    console.log("wrote", jsonPath, "root=", base);
  }
} finally {
  process.env = saved;
  await rm(base, { recursive: true, force: true });
}
