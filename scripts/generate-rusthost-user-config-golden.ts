/* oxlint-disable eslint(max-lines) -- 冻结脚本必须完整复刻测试的六个服务场景，拆分后更难与测试逐项对照。 */
// Phase 3 golden freeze：rustHostUserConfig.test.ts 的 TS 服务观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 录制范围与测试一一对应：output-style、memory、local broadcast、system extras、
// onboarding-record、model trajectory。归一化规则：
// - HOME 绝对路径写成 `$FIXTURE_ROOT`；
// - broadcast 的随机 token 写成 `<token>`；
// - probeIntranet 的 checkedAt/latency/端口与系统级连接错误写成占位符；
// - onboarding 的服务端时间写成 `<iso>`；
// - memory 中大文件内容写成 `<big>`。
import { deepEqual } from "node:assert/strict";
import { existsSync } from "node:fs";
import {
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  symlink,
  utimes,
  writeFile,
} from "node:fs/promises";
import { createServer } from "node:http";
import { createServer as createTcpServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  createBroadcastService,
  createMemoryService,
  createOnboardingRecordService,
  createOutputStyleService,
  createSystemService,
  readModelTrajectory,
  setDataBaseDir,
} from "../packages/services/src/node.ts";

type OutputStyleService = ReturnType<typeof createOutputStyleService>;
type MemoryService = ReturnType<typeof createMemoryService>;
type BroadcastService = ReturnType<typeof createBroadcastService>;
type OnboardingService = ReturnType<typeof createOnboardingRecordService>;

async function outcome<T>(run: () => Promise<T>) {
  try {
    return { ok: true as const, value: JSON.parse(JSON.stringify((await run()) ?? null)) };
  } catch (error) {
    const e = error as Error & { code?: string };
    return { ok: false as const, message: e.message, ...(e.code ? { code: e.code } : {}) };
  }
}

/** 目录树快照：相对路径 → 内容（JSON 文件解析后比较，不比键序）。 */
async function snapshot(root: string): Promise<Record<string, unknown>> {
  const result: Record<string, unknown> = {};
  const walk = async (dir: string) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        result[`${path.slice(root.length)}/`] = null;
        await walk(path);
      } else {
        const text = await readFile(path, "utf8");
        let content: unknown = text;
        if (path.endsWith(".json")) {
          try {
            content = JSON.parse(text);
          } catch {
            content = text;
          }
        }
        result[path.slice(root.length)] = content;
      }
    }
  };
  if (existsSync(root)) await walk(root);
  return result;
}

function normalize(value: unknown, home: string): unknown {
  return JSON.parse(
    JSON.stringify(value ?? null)
      .split(home)
      .join("$FIXTURE_ROOT"),
  );
}

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

const base = await realpath(await mkdtemp(join(tmpdir(), "rusthost-user-config-golden-")));
const previous = { HOME: process.env.HOME, USERPROFILE: process.env.USERPROFILE };
process.env.HOME = base;
delete process.env.USERPROFILE;

try {
  // ── output style ──────────────────────────────────────────────────────────
  const outputHome = join(base, "output-home");
  await mkdir(outputHome, { recursive: true });
  process.env.HOME = outputHome;
  setDataBaseDir(null);
  const outputStyle = createOutputStyleService();
  const styles = (home: string) => join(home, ".claude", "output-styles");
  const outputSteps: Array<[string, (s: OutputStyleService, home: string) => Promise<unknown>]> = [
    ["list without any directory", (s) => s.listStyles()],
    ["active style defaults to null", (s) => s.getActiveStyle()],
    ["user styles directory is created", (s) => s.getUserStylesDirectory()],
    [
      "add a style with irregular spacing",
      (s) =>
        s.addStyle({
          config: { name: "My  Cool\tStyle", description: "desc", content: "body\nline" },
        }),
    ],
    ["add Zeta", (s) => s.addStyle({ config: { name: "Zeta", description: "z", content: "" } })],
    ["add alpha", (s) => s.addStyle({ config: { name: "alpha", description: "a", content: "x" } })],
    [
      "raw files with edge-case front matter",
      async (_s, home) => {
        const dir = styles(home);
        await writeFile(join(dir, "crlf.md"), "name: Crlf Style\r\ndescription: D\r\n\r\nbody");
        await writeFile(join(dir, "plain.md"), "just text");
        await writeFile(join(dir, "gap.md"), "name:\nSecond Line Name\ndescription:   \n");
        await writeFile(join(dir, "x.mdy.md"), "name: Weird Id\n");
        await writeFile(join(dir, "ignored.txt"), "name: no\n");
        await mkdir(join(dir, "folder.md"));
        return null;
      },
    ],
    ["list with custom styles", (s) => s.listStyles()],
    ["activate a custom style", (s) => s.setActiveStyle({ styleId: "custom-my-cool-style" })],
    ["read the active style", (s) => s.getActiveStyle()],
    ["list reflects the activation", (s) => s.listStyles()],
    [
      "update a style",
      (s) =>
        s.updateStyle({
          id: "custom-zeta",
          config: { name: "Zeta 2", description: "changed", content: "new" },
        }),
    ],
    ["list after update", (s) => s.listStyles()],
    ["delete a style", (s) => s.deleteStyle({ id: "custom-alpha" })],
    ["delete a missing style", (s) => s.deleteStyle({ id: "custom-gone" })],
    ["activate a builtin", (s) => s.setActiveStyle({ styleId: "learning" })],
    [
      "settings.json keeps unrelated keys",
      async (_s, home) => {
        await writeFile(
          join(home, ".claude", "settings.json"),
          JSON.stringify({ hooks: { a: 1 }, outputStyle: "old", theme: "dark" }, null, 2),
        );
        return null;
      },
    ],
    ["activate again over existing settings", (s) => s.setActiveStyle({ styleId: "explanatory" })],
    [
      "corrupt settings.json is replaced",
      async (_s, home) => {
        await writeFile(join(home, ".claude", "settings.json"), "{ nope");
        return null;
      },
    ],
    ["activate over corrupt settings", (s) => s.setActiveStyle({ styleId: "default" })],
    ["final list", (s) => s.listStyles()],
  ];
  const outputStyleGolden = [];
  for (const [label, step] of outputSteps) {
    const result = await outcome(() => step(outputStyle, outputHome));
    outputStyleGolden.push({
      label,
      result: normalize(result, outputHome),
      files: normalize(await snapshot(outputHome), outputHome),
    });
  }

  // ── memory ────────────────────────────────────────────────────────────────
  const memoryHome = join(base, "memory-home");
  await mkdir(memoryHome, { recursive: true });
  process.env.HOME = memoryHome;
  setDataBaseDir(memoryHome);
  const memory = createMemoryService();
  const projects = (home: string) => join(home, ".escode", "cli", "memories", "projects");
  const memoryFixture = async (home: string) => {
    const root = projects(home);
    const at = (id: string) => join(root, id, "memory");
    for (const [id, files] of Object.entries({
      "alpha-0123456789abcdef": ["MEMORY.md", "b.md", "a.md", "notes.txt", "B.md"],
      beta: ["x.md"],
      "delta-bbbbbbbbbbbbbbbb": [],
    })) {
      await mkdir(at(id), { recursive: true });
      for (const file of files) await writeFile(join(at(id), file), `content of ${file}`);
    }
    await mkdir(join(root, "gamma-aaaaaaaaaaaaaaaa"), { recursive: true });
    await mkdir(join(at("alpha-0123456789abcdef"), "dir.md"));
    await symlink(join(at("beta"), "x.md"), join(at("alpha-0123456789abcdef"), "sym.md"));
    await symlink(join(root, "alpha-0123456789abcdef"), join(root, "linked"));
    await writeFile(join(at("beta"), "big.md"), Buffer.alloc(5 * 1024 * 1024 + 1, 97));
    await writeFile(join(at("beta"), "edge.md"), Buffer.alloc(5 * 1024 * 1024, 98));
    const times: Record<string, number> = {
      "alpha-0123456789abcdef/memory/MEMORY.md": 1_700_000_000.123,
      "alpha-0123456789abcdef/memory/b.md": 1_700_000_100.5,
      "alpha-0123456789abcdef/memory/a.md": 1_700_000_050,
      "alpha-0123456789abcdef/memory/B.md": 1_700_000_060,
      "alpha-0123456789abcdef/memory/notes.txt": 1_700_000_999,
      "beta/memory/x.md": 1_700_000_500,
      "beta/memory/big.md": 1_700_000_400,
      "beta/memory/edge.md": 1_700_000_450,
    };
    for (const [relative, seconds] of Object.entries(times)) {
      await utimes(
        join(root, relative.split("/")[0]!, ...relative.split("/").slice(1)),
        seconds,
        seconds,
      );
    }
  };
  const trimBig = (value: unknown) =>
    JSON.parse(
      JSON.stringify(value ?? null).replace(/"content":"[ab]{1000,}"/g, '"content":"<big>"'),
    );
  const readMemory = (workspaceId: string, fileName: string) => (s: MemoryService) =>
    s.readProjectMemoryFile({ workspaceId, fileName });
  const memorySteps: Array<[string, (s: MemoryService, home: string) => Promise<unknown>]> = [
    ["load before any save", (s) => s.loadMemory({ workspacePath: "/w", agentId: "claude" })],
    [
      "save user memory",
      (s) =>
        s.saveMemory({
          workspacePath: "/w",
          agentId: "claude",
          config: { content: "remember this", enabled: true },
        }),
    ],
    ["load after save", (s) => s.loadMemory({ workspacePath: "/w", agentId: "claude" })],
    ["clear user memory", (s) => s.clearMemory({ workspacePath: "/w", agentId: "claude" })],
    ["load after clear", (s) => s.loadMemory({ workspacePath: "/w", agentId: "claude" })],
    ["user memory directory", (s) => s.getUserMemoryDirectory()],
    ["project list without a root", (s) => s.listProjectMemories()],
    ["project read without a root", readMemory("alpha-0123456789abcdef", "MEMORY.md")],
    ["build project fixture", async (_s, home) => memoryFixture(home)],
    ["project list", (s) => s.listProjectMemories()],
    ["read index", readMemory("alpha-0123456789abcdef", "MEMORY.md")],
    ["read item", readMemory("alpha-0123456789abcdef", "a.md")],
    ["read other-case item", readMemory("alpha-0123456789abcdef", "B.md")],
    ["read wrong case", readMemory("alpha-0123456789abcdef", "A.md")],
    ["read traversal", readMemory("..", "MEMORY.md")],
    ["read nested file name", readMemory("alpha-0123456789abcdef", "a/b.md")],
    ["read non-markdown", readMemory("alpha-0123456789abcdef", "notes.txt")],
    ["read symlinked file", readMemory("alpha-0123456789abcdef", "sym.md")],
    ["read directory named like a file", readMemory("alpha-0123456789abcdef", "dir.md")],
    ["read missing file", readMemory("alpha-0123456789abcdef", "gone.md")],
    ["read missing workspace", readMemory("nowhere", "MEMORY.md")],
    ["read symlinked workspace", readMemory("linked", "MEMORY.md")],
    ["read over the preview limit", readMemory("beta", "big.md")],
    ["read exactly at the preview limit", readMemory("beta", "edge.md")],
  ];
  const memoryGolden = [];
  for (const [label, step] of memorySteps) {
    const result = trimBig(normalize(await outcome(() => step(memory, memoryHome)), memoryHome));
    memoryGolden.push({ label, result });
  }

  // ── broadcast (local mode) ────────────────────────────────────────────────
  setDataBaseDir(null);
  const broadcast = createBroadcastService(null);
  const broadcastMessages: unknown[] = [];
  const subscription = broadcast.onMessage((m) => broadcastMessages.push(m));
  await wait(300);
  const stripToken = (value: unknown) =>
    JSON.parse(JSON.stringify(value).replace(/"token":"[^"]+"/g, '"token":"<token>"'));
  const sends: unknown[] = [
    { channel: "state:theme", payload: { mode: "dark" } },
    { channel: "  state:locale  ", payload: "en", sourceWindowId: 7 },
    { channel: "no-payload" },
    { channel: "null-payload", payload: null },
    { channel: "   ", payload: 1 },
    { channel: "bad-id", payload: 1, sourceWindowId: 1.5 },
  ];
  const broadcastSends = [];
  for (const message of sends) {
    const result = await outcome(() => broadcast.send(message as never));
    broadcastSends.push({ message, ok: result.ok });
  }
  await wait(300);
  const runClaims = async (service: BroadcastService) => {
    const results: unknown[] = [];
    const record = async <T>(label: string, op: () => Promise<T>) => {
      results.push({ label, result: await op() });
    };
    let lease: { key: string; token: string } | undefined;
    await record("empty key", () => service.acquireClaim("   "));
    await record("acquire", async () => {
      const r = await service.acquireClaim(" claim-a ");
      if (r.status === "acquired") lease = r.lease;
      return r;
    });
    await record("busy", () => service.acquireClaim("claim-a"));
    await record("wrong token release", () =>
      service.releaseClaim({ key: "claim-a", token: "nope" }),
    );
    await record("still busy", () => service.acquireClaim("claim-a"));
    await record("release", () => service.releaseClaim(lease!));
    await record("re-acquire", async () => {
      const r = await service.acquireClaim("claim-a");
      if (r.status === "acquired") lease = r.lease;
      return r;
    });
    await record("commit", () => service.commitClaim(lease!));
    await record("committed", () => service.acquireClaim("claim-a"));
    await record("release after commit", () => service.releaseClaim(lease!));
    await record("still committed", () => service.acquireClaim("claim-a"));
    await record("tryClaim first", () => service.tryClaim("claim-b"));
    await record("tryClaim second", () => service.tryClaim("claim-b"));
    await record("tryClaim blank", () => service.tryClaim("  "));
    return results;
  };
  const broadcastGolden = {
    messages: stripToken(broadcastMessages),
    sends: broadcastSends,
    claims: stripToken(await runClaims(broadcast)),
  };
  subscription.dispose();

  // ── system extras ─────────────────────────────────────────────────────────
  const servers: Array<{ close(): void }> = [];
  const listen = (server: ReturnType<typeof createServer>) =>
    new Promise<number>((resolve) => {
      servers.push(server);
      server.listen(0, "127.0.0.1", () => resolve((server.address() as { port: number }).port));
    });
  const tcp = createTcpServer();
  servers.push(tcp);
  const tcpPort = await new Promise<number>((resolve) =>
    tcp.listen(0, "127.0.0.1", () => resolve((tcp.address() as { port: number }).port)),
  );
  const seenTokens: Array<string | undefined> = [];
  const httpPort = await listen(
    createServer((request, response) => {
      seenTokens.push(request.headers["x-zcode-intranet-token"] as string | undefined);
      const route = request.url ?? "/";
      if (route === "/ok") response.end(JSON.stringify({ ok: true, marker: "zcode" }));
      else if (route === "/notok") response.end(JSON.stringify({ ok: false }));
      else if (route === "/nomarker") response.end(JSON.stringify({ ok: true }));
      else if (route === "/bad") response.end("not json");
      else if (route === "/noflag") response.end(JSON.stringify({ marker: "x" }));
      else {
        response.statusCode = 503;
        response.end("{}");
      }
    }),
  );
  const system = createSystemService();
  const shells = await system.listIntegratedTerminalShells();
  const probeBase = `http://127.0.0.1:${httpPort}`;
  const probeRequests = [
    { targets: [{ host: "127.0.0.1", port: tcpPort }] },
    { targets: [{ host: "127.0.0.1", port: 1, timeoutMs: 100 }], attempts: 3 },
    {
      targets: [
        { host: " 127.0.0.1 ", port: tcpPort, id: " mine " },
        { host: "  " },
        { kind: "service", url: `${probeBase}/ok`, expectedMarker: "zcode", token: " tok " },
        { kind: "service", url: `${probeBase}/ok`, expectedMarker: "other" },
        { kind: "service", url: `${probeBase}/nomarker`, expectedMarker: "zcode" },
        { kind: "service", url: `${probeBase}/notok` },
        { kind: "service", url: `${probeBase}/bad` },
        { kind: "service", url: `${probeBase}/noflag` },
        { kind: "service", url: `${probeBase}/boom` },
        { kind: "service", url: "ftp://nope" },
      ],
      requiredSuccessCount: 3,
      attempts: 1,
    },
    { targets: [] },
    {
      targets: [{ host: "127.0.0.1", port: tcpPort }],
      requiredSuccessCount: 99,
      attempts: 99,
    },
  ];
  const normalizePorts = (value: unknown) =>
    JSON.parse(
      JSON.stringify(value)
        .split(`:${httpPort}`)
        .join(":$HTTP_PORT")
        .split(`"port":${tcpPort}`)
        .join('"port":"$TCP_PORT"')
        .split(`:${tcpPort}`)
        .join(":$TCP_PORT"),
    );
  const stripProbe = (
    value: {
      results: Array<Record<string, unknown>>;
    } & Record<string, unknown>,
  ) => ({
    ...value,
    checkedAt: "<t>",
    results: value.results.map((r) => {
      const { latencyMs, error, ...rest } = r;
      const keepError =
        typeof error === "string" &&
        /^(marker|service|HTTP|invalid service response:|timeout)/.test(error);
      return {
        ...rest,
        latency: latencyMs === null ? null : "<n>",
        ...(error === undefined ? {} : { error: keepError ? error : "<io-error>" }),
      };
    }),
  });
  const probes = [];
  for (const request of probeRequests) {
    const result = await system.probeIntranet(request as never);
    probes.push(stripProbe(normalizePorts(result) as never));
  }
  for (const server of servers) server.close();
  const systemGolden = { shells, probes };

  // ── onboarding-record ─────────────────────────────────────────────────────
  const onboardingHome = join(base, "onboarding-home");
  await mkdir(onboardingHome, { recursive: true });
  process.env.HOME = onboardingHome;
  setDataBaseDir(onboardingHome);
  const onboarding = createOnboardingRecordService({
    loadUserId: async () => null,
    hasExistingLocalTask: async () => false,
  });
  const entry = {
    occupation: "developer",
    interfaceMode: "coding",
    memoryEnabled: true,
    proactiveSuggestionsEnabled: null,
    completedAt: "2026-01-01T00:00:00.000Z",
  };
  const onboardingFile = (home: string) => join(home, ".escode", "v2", "onboarding-record.json");
  const redact = (value: unknown) =>
    JSON.parse(
      JSON.stringify(value ?? null).replace(
        /"decidedAt":"\d{4}-\d\d-\d\dT[\d:.]+Z"/g,
        '"decidedAt":"<iso>"',
      ),
    );
  const onboardingSteps: Array<[string, (s: OnboardingService) => Promise<unknown>]> = [
    ["records before anything", (s) => s.getRecords()],
    ["latest before anything", (s) => s.getLatestEntry()],
    ["sync before anything", (s) => s.syncSettingsFromRecord()],
    ["should onboard fresh", (s) => s.shouldOnboard("dev-1")],
    ["dismiss", (s) => s.dismissOnboarding("dev-1")],
    ["records after dismiss", async (s) => redact(await s.getRecords())],
    ["should onboard after dismiss", (s) => s.shouldOnboard("dev-1")],
    ["append", (s) => s.appendRecord("dev-2", entry as never)],
    ["records after append", (s) => s.getRecords()],
    ["latest", (s) => s.getLatestEntry()],
    ["sync", (s) => s.syncSettingsFromRecord()],
    [
      "append other occupation",
      (s) =>
        s.appendRecord("dev-3", {
          ...entry,
          occupation: "no-such-job",
          interfaceMode: null,
          memoryEnabled: null,
        } as never),
    ],
    ["sync with unknown occupation", (s) => s.syncSettingsFromRecord()],
    [
      "update preferences",
      (s) =>
        s.updateRecordPreferences({
          memoryEnabled: false,
          proactiveSuggestionsEnabled: true,
        }),
    ],
    ["records after update", (s) => s.getRecords()],
    ["claim is a no-op", (s) => s.claimAnonymousRecord()],
    ["dismiss with an entry is a no-op", (s) => s.dismissOnboarding("dev-1")],
    [
      "invalid append",
      (s) => s.appendRecord("dev-1", { ...entry, interfaceMode: "bogus" } as never),
    ],
    ["clear", (s) => s.clearRecords()],
    ["records after clear", (s) => s.getRecords()],
    ["update without a file", (s) => s.updateRecordPreferences({ memoryEnabled: true })],
  ];
  const onboardingGolden = [];
  for (const [label, step] of onboardingSteps) {
    const result = redact(normalize(await outcome(() => step(onboarding)), onboardingHome));
    const file = existsSync(onboardingFile(onboardingHome))
      ? redact(JSON.parse(await readFile(onboardingFile(onboardingHome), "utf8")))
      : null;
    onboardingGolden.push({ label, result, file });
  }

  // ── model trajectory ──────────────────────────────────────────────────────
  const modelHome = join(base, "model-home");
  await mkdir(modelHome, { recursive: true });
  process.env.HOME = modelHome;
  setDataBaseDir(modelHome);
  const line = (record: Record<string, unknown>) => JSON.stringify(record);
  const modelBase = (n: number, extra: Record<string, unknown> = {}) => ({
    type: "model_io",
    sessionId: "task-1",
    requestId: `req-${n}`,
    startedAt: `2026-01-01T00:00:0${n}.000Z`,
    model: { modelId: "m", providerId: "p", role: "main", source: "builtin" },
    ...extra,
  });
  const records = [
    modelBase(3, {
      request: {
        messagesKind: "delta",
        messageOffset: 1,
        messages: [
          { role: "user", content: "second" },
          { role: "tool", content: '{"ok":true}', toolCallId: "c1", name: "bash" },
          { role: "tool", content: "plain text", tool_use_id: "c2", is_error: true },
          { role: "tool", content: "[1,2]" },
          {
            role: "assistant",
            content: [
              { type: "text", text: "t" },
              { type: "reasoning", text: "r" },
              { type: "tool-call", toolCallId: "c1", toolName: "bash", args: { cmd: "ls" } },
              { type: "tool-result", result: 7 },
              { type: "image", mediaType: "image/png" },
              { type: "weird", extra: 1 },
              "raw-string",
            ],
          },
        ],
        toolNames: ["bash", 5, "read"],
      },
      response: {
        finishReason: "tool-calls",
        text: "done",
        reasoningText: "thinking",
        toolCalls: [
          { id: "c1", name: "bash", input: { cmd: "ls" } },
          { toolCallId: "c9", toolName: "x", args: 1 },
          "bad",
        ],
        usage: {
          inputTokens: 1,
          outputTokens: 2,
          totalTokens: 3,
          cacheReadTokens: 0,
          reasoningTokens: "x",
        },
        responseId: "resp",
        modelId: "m2",
      },
      completedAt: "2026-01-01T00:00:09.000Z",
      durationMs: 12.5,
      turnId: "turn",
      traceId: "trace",
      attempt: 2,
      querySource: "compact",
    }),
    modelBase(1, {
      request: {
        messages: [
          {
            role: "system",
            content: "Generate a concise title for this coding session. Foo",
          },
          { role: "user", content: "first" },
        ],
      },
      error: { name: "TypeError", message: "boom", stack: "at x" },
    }),
    modelBase(2, {
      startedAt: "2026-01-01T00:00:01.000Z",
      request: { messages: [{ role: "user", content: "same start" }] },
      model: { role: "subagent" },
    }),
    { ...modelBase(4), sessionId: "other" },
    { type: "not_model_io", sessionId: "task-1" },
    modelBase(5, { startedAt: "garbage", error: { message: "" } }),
  ];
  const body = `${records.map(line).join("\n")}\nnot json\n\n`;
  await mkdir(join(modelHome, ".escode", "cli", "debug"), { recursive: true });
  await mkdir(join(modelHome, ".escode", "cli", "rollout"), { recursive: true });
  await writeFile(join(modelHome, ".escode", "cli", "debug", "model-io-task-1.jsonl"), body);
  await writeFile(
    join(modelHome, ".escode", "cli", "rollout", "model-io-task-1.jsonl"),
    line(modelBase(6, { request: { messages: [{ role: "user", content: "rollout" }] } })),
  );
  await writeFile(
    join(modelHome, ".escode", "cli", "debug", "model-io-weird-id.jsonl"),
    line({ ...modelBase(1), sessionId: "weird id!" }),
  );
  const modelCases: Array<[string, { taskId: string; limit?: number }]> = [
    ["all", { taskId: "task-1" }],
    ["limited", { taskId: "task-1", limit: 2 }],
    ["fractional limit", { taskId: "task-1", limit: 2.9 }],
    ["invalid limit", { taskId: "task-1", limit: -4 }],
    ["sanitized id", { taskId: "weird id!" }],
    ["unknown task", { taskId: "ghost" }],
  ];
  const modelGoldenCases = [];
  for (const [label, params] of modelCases) {
    const result = normalize(
      await outcome(() => readModelTrajectory(params.taskId, params.limit)),
      modelHome,
    );
    modelGoldenCases.push({ label, result });
  }
  const filler =
    line(modelBase(7, { request: { messages: [{ role: "user", content: "x".repeat(2000) }] } })) +
    "\n";
  const big =
    filler.repeat(Math.ceil((33 * 1024 * 1024) / filler.length)) + line(modelBase(8)) + "\n";
  await writeFile(
    join(modelHome, ".escode", "cli", "debug", "model-io-big.jsonl"),
    big.replaceAll('"sessionId":"task-1"', '"sessionId":"big"'),
  );
  const pad =
    line({
      ...modelBase(7),
      sessionId: "pad",
      request: { messages: [{ role: "user", content: "y".repeat(2000) }] },
    }) + "\n";
  const quirk =
    pad.repeat(Math.ceil((33 * 1024 * 1024) / pad.length)) +
    Array.from({ length: 150 }, (_, i) =>
      line({
        ...modelBase(1),
        sessionId: "quirk",
        requestId: `q-${String(i).padStart(3, "0")}`,
        startedAt: `2026-02-01T00:00:00.${String(i).padStart(3, "0")}Z`,
      }),
    ).join("\n") +
    "\n";
  await writeFile(join(modelHome, ".escode", "cli", "debug", "model-io-quirk.jsonl"), quirk);
  const quirkResult = normalize(await outcome(() => readModelTrajectory("quirk")), modelHome) as {
    value: { records: unknown[] };
  };
  const bigResult = normalize(await outcome(() => readModelTrajectory("big", 5)), modelHome);

  const golden = {
    _readme:
      "rustHostUserConfig.test.ts 的 TS 服务观察冻结（Phase 3 golden freeze）。绝对临时根归一化为 $FIXTURE_ROOT；token/checkedAt/latency/端口/io 错误/decidedAt/大文件内容均已归一化。",
    outputStyle: outputStyleGolden,
    memory: memoryGolden,
    broadcast: broadcastGolden,
    system: systemGolden,
    onboarding: onboardingGolden,
    modelTrajectory: {
      cases: modelGoldenCases,
      quirk: quirkResult,
      quirkRecordCount: quirkResult.value.records.length,
      big: bigResult,
    },
  };

  const goldenDir = "packages/desktop/test/fixtures/golden";
  const goldenPath = join(goldenDir, "rustHostUserConfig.oracle.json");
  const json = `${JSON.stringify(golden, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    deepEqual(
      JSON.parse(await readFile(goldenPath, "utf8")),
      JSON.parse(json),
      "rustHostUserConfig golden 与当前 TS 观察漂移",
    );
    console.log("rustHostUserConfig golden: OK");
  } else {
    await mkdir(goldenDir, { recursive: true });
    await writeFile(goldenPath, json);
    console.log("wrote", goldenPath, "base=", base);
  }
} finally {
  setDataBaseDir(null);
  if (previous.HOME === undefined) delete process.env.HOME;
  else process.env.HOME = previous.HOME;
  if (previous.USERPROFILE !== undefined) process.env.USERPROFILE = previous.USERPROFILE;
  await rm(base, { recursive: true, force: true });
}
