import { describe, expect, it } from "vitest";
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
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  createBroadcastService,
  createMemoryService,
  createOutputStyleService,
  setDataBaseDir,
} from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-user-config-services.md：以 HOME 为根的用户配置目录类服务，对真实 TS 实现做同序操作差分。

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
      .join("<home>"),
  );
}

async function withHosts(
  run: (ctx: {
    tsHome: string;
    rustHome: string;
    rpc: Awaited<ReturnType<typeof connectRpc>>;
  }) => Promise<void>,
) {
  const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-userconfig-")));
  const tsHome = join(base, "ts-home");
  const rustHome = join(base, "rust-home");
  await mkdir(tsHome, { recursive: true });
  await mkdir(rustHome, { recursive: true });
  const previous = { HOME: process.env.HOME, USERPROFILE: process.env.USERPROFILE };
  process.env.HOME = tsHome;
  delete process.env.USERPROFILE;
  const host = startHost(rustHome);
  try {
    expect((await nextMeaningful(host)).kind).toBe("hostReady");
    host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home: rustHome, dataBaseDir: rustHome });
    const listening = await nextMeaningful(host);
    const rpc = await connectRpc(String(listening.addr), String(listening.secret));
    await run({ tsHome, rustHome, rpc });
  } finally {
    if (previous.HOME === undefined) delete process.env.HOME;
    else process.env.HOME = previous.HOME;
    if (previous.USERPROFILE !== undefined) process.env.USERPROFILE = previous.USERPROFILE;
    await host.stop();
    await rm(base, { recursive: true, force: true });
  }
}

describe("rust host user config services", () => {
  it.skipIf(!existsSync(hostBinary))(
    "output-style matches the TS service step by step",
    async () => {
      await withHosts(async ({ tsHome, rustHome, rpc }) => {
        const ts = createOutputStyleService();
        const rust = rpc.outputStyleService;
        const styles = (home: string) => join(home, ".claude", "output-styles");
        type Step = [string, (service: typeof ts, home: string) => Promise<unknown>];
        const steps: Step[] = [
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
          [
            "add Zeta",
            (s) => s.addStyle({ config: { name: "Zeta", description: "z", content: "" } }),
          ],
          [
            "add alpha",
            (s) => s.addStyle({ config: { name: "alpha", description: "a", content: "x" } }),
          ],
          [
            "raw files with edge-case front matter",
            async (_s, home) => {
              const dir = styles(home);
              await writeFile(
                join(dir, "crlf.md"),
                "name: Crlf Style\r\ndescription: D\r\n\r\nbody",
              );
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
          [
            "activate again over existing settings",
            (s) => s.setActiveStyle({ styleId: "explanatory" }),
          ],
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
        for (const [label, step] of steps) {
          const expected = await outcome(() => step(ts, tsHome));
          const actual = await outcome(() => step(rust, rustHome));
          expect({ label, result: normalize(actual, rustHome) }).toEqual({
            label,
            result: normalize(expected, tsHome),
          });
          expect({ label, files: normalize(await snapshot(rustHome), rustHome) }).toEqual({
            label,
            files: normalize(await snapshot(tsHome), tsHome),
          });
        }
      });
    },
    120_000,
  );

  it.skipIf(!existsSync(hostBinary))(
    "memory matches the TS service for user memory and the project memory views",
    async () => {
      await withHosts(async ({ tsHome, rustHome, rpc }) => {
        setDataBaseDir(tsHome);
        try {
          const ts = createMemoryService();
          const rust = rpc.memoryService;
          const projects = (home: string) => join(home, ".escode", "cli", "memories", "projects");
          const fixture = async (home: string) => {
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
          type Step = [string, (service: typeof ts, home: string) => Promise<unknown>];
          const read =
            (workspaceId: string, fileName: string): Step[1] =>
            (s) =>
              s.readProjectMemoryFile({ workspaceId, fileName });
          const steps: Step[] = [
            [
              "load before any save",
              (s) => s.loadMemory({ workspacePath: "/w", agentId: "claude" }),
            ],
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
            ["project read without a root", read("alpha-0123456789abcdef", "MEMORY.md")],
            ["build project fixture", async (_s, home) => fixture(home)],
            ["project list", (s) => s.listProjectMemories()],
            ["read index", read("alpha-0123456789abcdef", "MEMORY.md")],
            ["read item", read("alpha-0123456789abcdef", "a.md")],
            ["read other-case item", read("alpha-0123456789abcdef", "B.md")],
            ["read wrong case", read("alpha-0123456789abcdef", "A.md")],
            ["read traversal", read("..", "MEMORY.md")],
            ["read nested file name", read("alpha-0123456789abcdef", "a/b.md")],
            ["read non-markdown", read("alpha-0123456789abcdef", "notes.txt")],
            ["read symlinked file", read("alpha-0123456789abcdef", "sym.md")],
            ["read directory named like a file", read("alpha-0123456789abcdef", "dir.md")],
            ["read missing file", read("alpha-0123456789abcdef", "gone.md")],
            ["read missing workspace", read("nowhere", "MEMORY.md")],
            ["read symlinked workspace", read("linked", "MEMORY.md")],
            ["read over the preview limit", read("beta", "big.md")],
            ["read exactly at the preview limit", read("beta", "edge.md")],
          ];
          for (const [label, step] of steps) {
            const expected = await outcome(() => step(ts, tsHome));
            const actual = await outcome(() => step(rust, rustHome));
            const trimBig = (value: unknown) =>
              JSON.parse(
                JSON.stringify(value ?? null).replace(
                  /"content":"[ab]{1000,}"/g,
                  '"content":"<big>"',
                ),
              );
            expect({ label, result: trimBig(normalize(actual, rustHome)) }).toEqual({
              label,
              result: trimBig(normalize(expected, tsHome)),
            });
          }
        } finally {
          setDataBaseDir(null);
        }
      });
    },
    120_000,
  );
});

describe("rust host broadcast (local mode)", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the TS local broadcast service",
    async () => {
      await withHosts(async ({ rpc }) => {
        const ts = createBroadcastService(null);
        const rust = rpc.broadcastService;
        const tsMessages: unknown[] = [];
        const rustMessages: unknown[] = [];
        const subscriptions = [
          ts.onMessage((m) => tsMessages.push(m)),
          rust.onMessage((m) => rustMessages.push(m)),
        ];
        await new Promise((r) => setTimeout(r, 300));

        const strip = (value: unknown) =>
          JSON.parse(JSON.stringify(value).replace(/"token":"[^"]+"/g, '"token":"<token>"'));
        const sends: unknown[] = [
          { channel: "state:theme", payload: { mode: "dark" } },
          { channel: "  state:locale  ", payload: "en", sourceWindowId: 7 },
          { channel: "no-payload" },
          { channel: "null-payload", payload: null },
          { channel: "   ", payload: 1 },
          { channel: "bad-id", payload: 1, sourceWindowId: 1.5 },
        ];
        for (const message of sends) {
          const expected = await outcome(() => ts.send(message as never));
          const actual = await outcome(() => rust.send(message as never));
          expect({ message, ok: actual.ok }).toEqual({ message, ok: expected.ok });
        }
        await new Promise((r) => setTimeout(r, 300));
        expect(strip(rustMessages)).toEqual(strip(tsMessages));
        expect(tsMessages).toHaveLength(3);

        // claim 状态机：两边同序执行，结果去掉随机 token 后相等，且 token 可用于后续 commit/release。
        const run = async (service: typeof ts) => {
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
        expect(strip(await run(rust))).toEqual(strip(await run(ts)));
        for (const subscription of subscriptions) subscription.dispose();
      });
    },
    120_000,
  );
});

describe("rust host system extras", () => {
  it.skipIf(!existsSync(hostBinary))(
    "listIntegratedTerminalShells and probeIntranet match the TS system service",
    async () => {
      const { createServer } = await import("node:http");
      const { createServer: createTcpServer } = await import("node:net");
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
      const { createSystemService } = await import("@escode/services/node");
      const ts = createSystemService();
      await withHosts(async ({ rpc }) => {
        const rust = rpc.systemService;
        expect(await rust.listIntegratedTerminalShells()).toEqual(
          await ts.listIntegratedTerminalShells(),
        );
        const base = `http://127.0.0.1:${httpPort}`;
        const requests = [
          { targets: [{ host: "127.0.0.1", port: tcpPort }] },
          { targets: [{ host: "127.0.0.1", port: 1, timeoutMs: 100 }], attempts: 3 },
          {
            targets: [
              { host: " 127.0.0.1 ", port: tcpPort, id: " mine " },
              { host: "  " },
              { kind: "service", url: `${base}/ok`, expectedMarker: "zcode", token: " tok " },
              { kind: "service", url: `${base}/ok`, expectedMarker: "other" },
              { kind: "service", url: `${base}/nomarker`, expectedMarker: "zcode" },
              { kind: "service", url: `${base}/notok` },
              { kind: "service", url: `${base}/bad` },
              { kind: "service", url: `${base}/noflag` },
              { kind: "service", url: `${base}/boom` },
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
        const strip = (
          value: { results: Array<Record<string, unknown>> } & Record<string, unknown>,
        ) => ({
          ...value,
          checkedAt: "<t>",
          results: value.results.map((r) => {
            const { latencyMs, error, ...rest } = r;
            // 系统级连接错误文案（ECONNREFUSED 等）因运行时而异，只比较其是否存在。
            const keepError =
              typeof error === "string" && /^(marker|service|HTTP|invalid service response:|timeout)/.test(error);
            return {
              ...rest,
              latency: latencyMs === null ? null : "<n>",
              ...(error === undefined ? {} : { error: keepError ? error : "<io-error>" }),
            };
          }),
        });
        for (const request of requests) {
          expect({
            request,
            result: strip((await rust.probeIntranet(request as never)) as never),
          }).toEqual({
            request,
            result: strip((await ts.probeIntranet(request as never)) as never),
          });
        }
        expect(seenTokens.filter(Boolean).length).toBeGreaterThanOrEqual(2);
        expect(new Set(seenTokens.filter(Boolean))).toEqual(new Set(["tok"]));
      });
      for (const server of servers) server.close();
    },
    120_000,
  );
});
