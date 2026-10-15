import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createProviderRuntime } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-providers.md P1 TS oracle 差分：同一对配置文件（真实 Built-in + 各种 Personal）分别交给真实 TS
// `ProviderRuntime.modelSelection` 与 Rust Host 的 `model-selection` channel，`getView` 逐项全等。

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");

const keyed = (id: string, extra: Record<string, unknown> = {}) => ({
  providerId: id,
  templateId: "openai",
  config: {
    group: "standard-personal",
    access: { type: "api-key", apiKey: "sk-test" },
    ...extra,
  },
});

function personalFile(config: {
  providers?: unknown[];
  providerOrder?: string[];
  providerModelRules?: unknown[];
  manualProviderModelRules?: unknown[];
  defaultModelSelection?: unknown;
}) {
  return {
    schemaVersion: 1,
    config: {
      ...(config.providerOrder ? { providerOrder: config.providerOrder } : {}),
      providerConfigRules: { providerRules: config.providers ?? [] },
      modelConfigRules: {
        providerModelRules: config.providerModelRules ?? [],
        manualProviderModelRules: config.manualProviderModelRules ?? [],
      },
      ...(config.defaultModelSelection
        ? { defaultModelSelection: config.defaultModelSelection }
        : {}),
    },
  };
}

const manualConfig = {
  enabled: true,
  properties: {
    contextWindow: 64000,
    supportsJsonSchemaOutput: true,
    supportsNativeWebSearch: false,
    supportsMidConversationSystem: false,
    inputFormat: { supportsImage: true, supportsVideo: false, supportsPdf: false },
  },
  optionSpecs: {
    reasoningLevel: { values: ["fast", "deep"], map: "{'effort': reasoningLevel}" },
    maxOutputTokens: { max: 8192 },
  },
};

const PROVIDERS = [
  keyed("p-openai", { personalModelIds: ["my-model"], modelOrder: ["my-model", "gpt-5.6"] }),
  { ...keyed("p-kimi"), templateId: "moonshot-kimi" },
  { ...keyed("p-deepseek"), templateId: "deepseek", enabled: false },
  keyed("p-hidden", { visibility: "hidden" }),
  keyed("p-incomplete", { access: { type: "api-key", apiKey: "   " } }),
  {
    providerId: "p-bad-url",
    config: {
      group: "standard-personal",
      access: { type: "api-key", apiKey: "k" },
      api: { type: "openai-responses", baseUrl: "not a url" },
      personalModelIds: ["m"],
    },
  },
  {
    providerId: "p-no-api",
    config: {
      group: "standard-personal",
      access: { type: "api-key", apiKey: "k" },
      personalModelIds: ["m"],
    },
  },
  { ...keyed("p-ghost-template"), templateId: "ghost" },
  {
    ...keyed("p-no-group"),
    config: { access: { type: "api-key", apiKey: "k" }, personalModelIds: ["m"] },
  },
  {
    providerId: "p-custom",
    config: {
      group: "standard-personal",
      access: { type: "api-key", apiKey: "k" },
      api: {
        type: "openai-chat-completions",
        baseUrl: "https://llm.example.com/v1/",
        headers: { "x-a": "1" },
      },
      personalModelIds: ["glm-5.3-flash", "other"],
    },
  },
];

const SCENARIOS: Array<[string, unknown]> = [
  ["no personal file", null],
  ["empty personal", personalFile({})],
  ["providers", personalFile({ providers: PROVIDERS })],
  ["provider order", personalFile({ providers: PROVIDERS, providerOrder: ["p-custom", "p-kimi"] })],
  [
    "manual rule",
    personalFile({
      providers: PROVIDERS,
      manualProviderModelRules: [
        { providerId: "p-openai", modelId: "my-model", config: manualConfig },
      ],
    }),
  ],
  [
    "smart rule disabling a model",
    personalFile({
      providers: PROVIDERS,
      providerModelRules: [
        { providerId: "p-openai", modelId: "gpt-5.6", config: { enabled: false } },
      ],
    }),
  ],
  [
    "default selection valid",
    personalFile({
      providers: PROVIDERS,
      defaultModelSelection: {
        providerId: "p-kimi",
        modelId: "kimi-k3",
        options: { reasoningLevel: "high" },
      },
    }),
  ],
  [
    "default selection unknown provider",
    personalFile({
      providers: PROVIDERS,
      defaultModelSelection: {
        providerId: "ghost",
        modelId: "x",
        options: { reasoningLevel: "high" },
      },
    }),
  ],
  [
    "default selection bad level",
    personalFile({
      providers: PROVIDERS,
      defaultModelSelection: {
        providerId: "p-kimi",
        modelId: "kimi-k3",
        options: { reasoningLevel: "nope" },
      },
    }),
  ],
  [
    "default selection missing level",
    personalFile({
      providers: PROVIDERS,
      defaultModelSelection: { providerId: "p-kimi", modelId: "kimi-k3" },
    }),
  ],
  [
    "default selection hidden provider",
    personalFile({
      providers: PROVIDERS,
      defaultModelSelection: {
        providerId: "p-hidden",
        modelId: "gpt-5.6",
        options: { reasoningLevel: "high" },
      },
    }),
  ],
  [
    "default selection legacy off",
    personalFile({
      providers: PROVIDERS,
      defaultModelSelection: {
        providerId: "p-kimi",
        modelId: "kimi-k2.6",
        options: { reasoningLevel: "off" },
      },
    }),
  ],
  [
    "legacy off blocked by a personal override of the levels",
    personalFile({
      providers: PROVIDERS,
      providerModelRules: [
        {
          providerId: "p-kimi",
          modelId: "kimi-k2.6",
          config: { optionSpecs: { reasoningLevel: { values: ["disabled", "enabled"] } } },
        },
      ],
      defaultModelSelection: {
        providerId: "p-kimi",
        modelId: "kimi-k2.6",
        options: { reasoningLevel: "off" },
      },
    }),
  ],
  [
    "extra providers: glm and deepseek",
    personalFile({
      providers: [
        { ...keyed("p-glm"), templateId: "zai-api" },
        { ...keyed("p-ds"), templateId: "deepseek" },
      ],
    }),
  ],
  ["invalid personal file falls back to empty", { schemaVersion: 1, config: { surprise: true } }],
  ["unsupported personal version", { schemaVersion: 2, config: {} }],
];

const INPUTS: Array<unknown> = [
  undefined,
  { selection: null },
  {
    selection: { providerId: "p-openai", modelId: "gpt-5.6", options: { reasoningLevel: "high" } },
  },
  { selection: { providerId: "p-openai", modelId: "gpt-5.6" } },
  {
    selection: { providerId: "p-openai", modelId: "gpt-5.6", options: { reasoningLevel: "bogus" } },
  },
  { selection: { providerId: "p-openai", modelId: "nope" } },
  { selection: { providerId: "ghost", modelId: "nope" } },
  {
    selection: { providerId: "p-hidden", modelId: "gpt-5.6", options: { reasoningLevel: "high" } },
  },
  { selection: { providerId: "account:zai-start-plan", modelId: "GLM-5.3-Flash" } },
  { selection: { providerId: "account:zai-offpeak-idle-plan", modelId: "GLM-5.3" } },
  { selection: { providerId: "account:zai-individual-coding-plan", modelId: "GLM-5.3" } },
  { selection: { providerId: "account:bigmodel-team-coding-plan", modelId: "GLM-5.3-Flash" } },
  { selection: { providerId: "p-kimi", modelId: "kimi-k2.6", options: { reasoningLevel: "off" } } },
  {
    selection: {
      providerId: "p-kimi",
      modelId: "kimi-k2.6",
      options: { reasoningLevel: "enabled" },
    },
  },
  { selection: { providerId: "p-kimi", modelId: "kimi-k3", options: { reasoningLevel: "off" } } },
  {
    selection: {
      providerId: "p-ds",
      modelId: "deepseek-v4-pro",
      options: { reasoningLevel: "off" },
    },
  },
  {
    selection: { providerId: "p-glm", modelId: "GLM-5.3", options: { reasoningLevel: "nothink" } },
  },
  { selection: { providerId: "p-deepseek", modelId: "deepseek-flash" } },
];

describe("rust host providers P1", () => {
  it.skipIf(!existsSync(hostBinary) || !existsSync(BUILTIN_SOURCE))(
    "matches the TS ModelSelection view for the same provider config files",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-provider-")));
      const builtin = join(base, "escode-builtin.json");
      const personal = join(base, "provider_config.json");
      await copyFile(BUILTIN_SOURCE, builtin);
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
        expect(listening.kind).toBe("rpcListening");
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const rust = services.modelSelectionService;

        let previousRevision = 0;
        for (const [label, content] of SCENARIOS) {
          if (content === null) await rm(personal, { force: true });
          else await writeFile(personal, JSON.stringify(content, null, 2));

          const runtime = createProviderRuntime({
            escodeBuiltinFilePath: builtin,
            personalFilePath: personal,
            watch: false,
            personalPollingIntervalMs: false,
          });
          try {
            const settingsExpected = await runtime.providerSettings.getView();
            const settingsActual = await services.providerSettingsService.getView();
            const strip = (view: { revision: number }) => ({ ...view, revision: "<n>" });
            expect({ label, settings: strip(settingsActual) }).toEqual({
              label,
              settings: strip(JSON.parse(JSON.stringify(settingsExpected))),
            });
            const refreshed = await services.providerSettingsService.refresh("test");
            expect(strip(refreshed)).toEqual(strip(settingsActual));
            for (const input of INPUTS) {
              const expected = await runtime.modelSelection.getView(input as never);
              const actual = await rust.getView(input as never);
              expect({ label, input, view: strip(actual) }).toEqual({
                label,
                input,
                view: strip(JSON.parse(JSON.stringify(expected))),
              });
              if (input === undefined) {
                expect(actual.revision, `${label}: 修订号随配置变化单调递增`).toBeGreaterThan(
                  previousRevision,
                );
                previousRevision = actual.revision;
              }
            }
          } finally {
            runtime.dispose();
          }
        }
      } finally {
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    300_000,
  );
});
