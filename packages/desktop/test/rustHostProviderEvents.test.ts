import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-providers.md §9：`onDidChange` 事件——外部改文件与本进程写入都会推送新算出的视图，
// 订阅瞬间的初始快照不算变化。

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");

const personalWith = (providerName: string) => ({
  schemaVersion: 1,
  config: {
    providerConfigRules: {
      providerRules: [
        {
          providerId: "p-events",
          templateId: "openai",
          providerName,
          config: {
            group: "standard-personal",
            access: { type: "api-key", apiKey: "sk-test" },
            personalModelIds: ["m1"],
          },
        },
      ],
    },
    modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
  },
});

type View = { revision: number; providers: Array<{ providerId: string; providerName?: string }> };

function collect<T>(subscribe: (listener: (event: T) => void) => { dispose(): void }) {
  const events: T[] = [];
  const subscription = subscribe((event) => events.push(event));
  const waitFor = async (predicate: (events: T[]) => boolean, what: string) => {
    const deadline = Date.now() + 10_000;
    while (!predicate(events)) {
      if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
      await new Promise((r) => setTimeout(r, 25));
    }
  };
  return { events, waitFor, dispose: () => subscription.dispose() };
}

describe("rust host providers P4 events", () => {
  it.skipIf(!existsSync(hostBinary) || !existsSync(BUILTIN_SOURCE))(
    "pushes a fresh view when the personal file changes externally or through a mutation",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-provider-ev-")));
      const builtin = join(base, "escode-builtin.json");
      const personal = join(base, "provider_config.json");
      await copyFile(BUILTIN_SOURCE, builtin);
      await writeFile(personal, JSON.stringify(personalWith("First"), null, 2));
      const home = join(base, "home");
      const host = startHost(home);
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({
          kind: "rpcListen",
          bind: "127.0.0.1:0",
          home,
          runtimeEnv: {
            ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtin,
            ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: personal,
          },
        });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const settings = services.providerSettingsService;
        const selection = services.modelSelectionService;

        const settingsEvents = collect<View>((l) => settings.onDidChange(l as never));
        const selectionEvents = collect<View>((l) => selection.onDidChange(l as never));
        // 事件订阅是异步建立的；先做一次往返，确保 Host 已登记监听并完成初始加载。
        const initial = (await settings.getView()) as unknown as View;
        await new Promise((r) => setTimeout(r, 1500));
        expect(settingsEvents.events, "初始快照不算变化").toHaveLength(0);

        // 外部改文件 → 轮询发现 → 推送新视图。
        await writeFile(personal, JSON.stringify(personalWith("Second"), null, 2));
        await settingsEvents.waitFor((e) => e.length >= 1, "external settings event");
        await selectionEvents.waitFor((e) => e.length >= 1, "external selection event");
        const external = settingsEvents.events.at(-1)!;
        expect(external.revision).toBeGreaterThan(initial.revision);
        expect(external.providers.find((p) => p.providerId === "p-events")?.providerName).toBe(
          "Second",
        );

        // 本进程写入：返回的视图与推送的视图同一修订。
        const before = settingsEvents.events.length;
        const returned = (await settings.createPersonalProvider({
          providerName: "Third",
        })) as unknown as { providerId: string; view: View };
        await settingsEvents.waitFor((e) => e.length > before, "mutation event");
        expect(settingsEvents.events.at(-1)!.revision).toBe(returned.view.revision);
        expect(
          settingsEvents.events.at(-1)!.providers.some((p) => p.providerId === returned.providerId),
        ).toBe(true);

        settingsEvents.dispose();
        selectionEvents.dispose();
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
