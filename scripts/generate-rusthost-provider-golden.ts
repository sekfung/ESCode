// Phase 3 golden freeze：rustHostProvider.test.ts 的 TS `createProviderRuntime` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：临时根路径写成 `$FIXTURE_ROOT`（测试运行时替换）；Registry revision 是进程内单调计数，
// 统一写成 `<n>`，同时保留测试中的跨场景单调递增断言。
import { deepEqual } from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { gzipSync, gunzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createProviderRuntime } from "../packages/services/src/node.ts";

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

const strip = (view: { revision: number }) => ({ ...view, revision: "<n>" });
const normalize = (value: unknown, root: string): unknown =>
  JSON.parse(JSON.stringify(value).split(root).join("$FIXTURE_ROOT"));

const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-provider-golden-")));
const builtin = join(base, "escode-builtin.json");
const personal = join(base, "provider_config.json");
await copyFile(BUILTIN_SOURCE, builtin);

const scenarios: Array<Record<string, unknown>> = [];
try {
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
      const settings = strip(JSON.parse(JSON.stringify(await runtime.providerSettings.getView())));
      const views: unknown[] = [];
      for (const input of INPUTS) {
        const view = await runtime.modelSelection.getView(input as never);
        views.push(strip(JSON.parse(JSON.stringify(view))));
      }
      scenarios.push({
        label,
        settings: normalize(settings, base),
        views: normalize(views, base),
      });
    } finally {
      runtime.dispose();
    }
  }
} finally {
  await rm(base, { recursive: true, force: true });
}

const golden = {
  _readme:
    "rustHostProvider.test.ts 的 TS createProviderRuntime 观察冻结（Phase 3 golden freeze）。revision 归一化为 <n>；绝对临时路径归一化为 $FIXTURE_ROOT，测试运行时替换为本次工作根。",
  scenarios,
};
const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenPath = join(goldenDir, "rustHostProvider.oracle.json.gz");
const json = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(gunzipSync(await readFile(goldenPath)).toString("utf8")),
    JSON.parse(json),
    "rustHostProvider golden 与当前 TS 观察漂移",
  );
  console.log("rustHostProvider golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, gzipSync(Buffer.from(json)));
  console.log("wrote", goldenPath);
}
