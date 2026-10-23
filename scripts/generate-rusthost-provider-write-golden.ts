/* oxlint-disable eslint(max-lines) -- 冻结脚本必须完整复刻测试的 OPS 序列，拆分后更难与测试逐项对照。 */
// Phase 3 golden freeze：rustHostProviderWrite.test.ts 的 TS `createProviderRuntime.providerSettings` 写操作观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 归一化：revision 统一写成 `<n>`（与测试 strip 一致）；错误文案仅在测试原本就比较文案时冻结
// （`anyMessage` 的用例只比较成功/失败）；临时根路径写成 `$FIXTURE_ROOT`，测试运行时替换。
import { deepEqual } from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { gzipSync, gunzipSync } from "node:zlib";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createProviderRuntime } from "../packages/services/src/node.ts";

const BUILTIN_SOURCE = resolve("config/provider/escode-builtin.json");

type Api = {
  getView(): Promise<{ revision: number; [key: string]: unknown }>;
  [method: string]: (...args: never[]) => Promise<unknown>;
};

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

const key = { type: "api-key", apiKey: "sk-test" };
const INITIAL = {
  schemaVersion: 1,
  config: {
    providerOrder: ["p-custom", "p-openai"],
    providerConfigRules: {
      providerRules: [
        {
          providerId: "p-openai",
          templateId: "openai",
          providerName: "My OpenAI",
          config: {
            group: "standard-personal",
            access: key,
            personalModelIds: ["my-model", "other"],
            modelOrder: ["my-model", "gpt-5.6"],
          },
        },
        {
          providerId: "p-custom",
          providerName: "Custom",
          config: {
            group: "standard-personal",
            access: key,
            api: { type: "openai-chat-completions", baseUrl: "https://llm.example.com/v1" },
            personalModelIds: ["c1", "c2"],
          },
        },
        { providerId: "account:zai-start-plan", config: { visibility: "visible" } },
      ],
    },
    modelConfigRules: {
      providerModelRules: [
        { providerId: "p-openai", modelId: "gpt-5.6", config: { enabled: false } },
        { providerId: "p-openai", modelId: "other", config: { enabled: true } },
      ],
      manualProviderModelRules: [
        { providerId: "p-openai", modelId: "my-model", config: manualConfig },
      ],
    },
    defaultModelSelection: { providerId: "p-openai", modelId: "gpt-5.6" },
  },
};

type Op = [string, (api: Api) => Promise<unknown>, { anyMessage?: boolean }?];

const OPS: Op[] = [
  [
    "disable personal model",
    (a) => a.setPersonalModelEnabled("p-openai", "my-model", false as never),
  ],
  [
    "enable inherited model",
    (a) => a.setPersonalModelEnabled("p-openai", "gpt-5.4", true as never),
  ],
  ["enable unknown model", (a) => a.setPersonalModelEnabled("p-openai", "nope", true as never)],
  ["enable on unknown provider", (a) => a.setPersonalModelEnabled("ghost", "x", true as never)],
  ["add personal model", (a) => a.addPersonalModel("p-custom", "c3", {} as never)],
  ["add duplicate personal model", (a) => a.addPersonalModel("p-custom", "c3", {} as never)],
  ["add inherited duplicate", (a) => a.addPersonalModel("p-openai", "gpt-5.6", {} as never)],
  [
    "add manual model",
    (a) => a.addPersonalModel("p-custom", "c4", manualConfig as never, false as never),
  ],
  [
    "add model with incomplete manual config",
    (a) => a.addPersonalModel("p-custom", "c5", { enabled: true } as never, false as never),
    { anyMessage: true },
  ],
  ["add model with trimmed ids", (a) => a.addPersonalModel("  p-custom ", " c6 ", {} as never)],
  ["rename personal model", (a) => a.renamePersonalModel("p-custom", "c1", "c1-renamed")],
  ["rename to the same id", (a) => a.renamePersonalModel("p-custom", "c2", "c2")],
  ["rename inherited model", (a) => a.renamePersonalModel("p-openai", "gpt-5.6", "zzz")],
  ["rename onto existing", (a) => a.renamePersonalModel("p-custom", "c2", "c3")],
  ["rename onto inherited", (a) => a.renamePersonalModel("p-openai", "my-model", "gpt-5.4")],
  ["rename missing", (a) => a.renamePersonalModel("p-custom", "nope", "other")],
  ["delete personal model", (a) => a.deletePersonalModel("p-custom", "c3")],
  ["delete inherited model", (a) => a.deletePersonalModel("p-openai", "gpt-5.6")],
  ["delete missing model", (a) => a.deletePersonalModel("p-custom", "nope")],
  [
    "reorder models",
    (a) => a.reorderPersonalModels("p-openai", ["other", "gpt-5.4", "my-model", "ghost", "other"]),
  ],
  [
    "reorder models on an inherited-only overlay",
    (a) => a.reorderPersonalModels("account:zai-start-plan", ["GLM-5.2"]),
  ],
  ["reorder models of an unknown provider", (a) => a.reorderPersonalModels("ghost", ["x"])],
  ["reorder providers", (a) => a.reorderPersonalProviders(["p-openai", "ghost", "p-custom"])],
  ["create bare provider", (a) => a.createPersonalProvider()],
  ["create bare provider again", (a) => a.createPersonalProvider()],
  ["create from template", (a) => a.createPersonalProvider({ templateId: "openai" })],
  ["create from template again", (a) => a.createPersonalProvider({ templateId: "openai" })],
  [
    "create zh-CN template",
    (a) => a.createPersonalProvider({ templateId: "moonshot-kimi", locale: "zh-CN" }),
  ],
  ["create with a duplicate name", (a) => a.createPersonalProvider({ providerName: "custom" })],
  ["create from an unknown template", (a) => a.createPersonalProvider({ templateId: "ghost" })],
  [
    "create with initial group",
    (a) => a.createPersonalProvider({ initialConfig: { group: "standard-personal" } }),
  ],
  [
    "create with initial config",
    (a) =>
      a.createPersonalProvider({
        templateId: "deepseek",
        providerName: "DS",
        initialConfig: { access: { type: "api-key", apiKey: "k" }, visibility: "hidden" },
      }),
  ],
  [
    "save overlay: api change",
    (a) =>
      a.savePersonalProviderOverlay("p-custom", {
        group: "standard-personal",
        access: key,
        api: { type: "openai-responses", baseUrl: "https://new.example.com/v1" },
      } as never),
  ],
  [
    "save overlay: rename provider",
    (a) =>
      a.savePersonalProviderOverlay(
        "p-custom",
        { group: "standard-personal" } as never,
        {
          providerName: "  Renamed  ",
        } as never,
      ),
  ],
  [
    "save overlay: duplicate name",
    (a) =>
      a.savePersonalProviderOverlay(
        "p-custom",
        { group: "standard-personal" } as never,
        {
          providerName: "my openai",
        } as never,
      ),
  ],
  [
    "save overlay: clear name",
    (a) =>
      a.savePersonalProviderOverlay(
        "p-custom",
        { group: "standard-personal" } as never,
        {
          providerName: null,
        } as never,
      ),
  ],
  [
    "save overlay: personal-only without group",
    (a) => a.savePersonalProviderOverlay("p-custom", { visibility: "hidden" } as never),
  ],
  [
    "save overlay: wrong group on personal-only",
    (a) => a.savePersonalProviderOverlay("p-custom", { group: "zai-family" } as never),
  ],
  [
    "save overlay: builtin overlay visibility",
    (a) =>
      a.savePersonalProviderOverlay("account:zai-start-plan", { visibility: "hidden" } as never),
  ],
  [
    "save overlay: builtin overlay group rewrite",
    (a) =>
      a.savePersonalProviderOverlay("account:zai-start-plan", {
        group: "bigmodel-family",
      } as never),
  ],
  [
    "save overlay: builtin overlay group same",
    (a) =>
      a.savePersonalProviderOverlay("account:zai-start-plan", { group: "zai-family" } as never),
  ],
  [
    "save overlay: account provider access",
    (a) => a.savePersonalProviderOverlay("account:zai-start-plan", { access: key } as never),
  ],
  [
    "save overlay: account provider disable",
    (a) =>
      a.savePersonalProviderOverlay(
        "account:zai-start-plan",
        {} as never,
        {
          enabled: false,
        } as never,
      ),
  ],
  [
    "save overlay: metadata enabled and template",
    (a) =>
      a.savePersonalProviderOverlay(
        "p-openai",
        { group: "standard-personal", access: key } as never,
        {
          enabled: false,
          templateId: "openai",
        } as never,
      ),
  ],
  ["save overlay: uncreated provider", (a) => a.savePersonalProviderOverlay("ghost", {} as never)],
  [
    "save overlay: unknown field",
    (a) => a.savePersonalProviderOverlay("p-custom", { surprise: 1 } as never),
    { anyMessage: true },
  ],
  [
    "draft: edit config in place",
    async (a) => {
      const view = await a.getView();
      return a.savePersonalModelDraft({
        providerId: "p-openai",
        originalModelId: "other",
        nextModelId: "other",
        personalConfig: { enabled: false },
        basedOnRevision: view.revision,
      } as never);
    },
  ],
  [
    "draft: rename and recommended-empty clears the rule",
    async (a) => {
      const view = await a.getView();
      return a.savePersonalModelDraft({
        providerId: "p-openai",
        originalModelId: "other",
        nextModelId: "renamed",
        personalConfig: {},
        useRecommendedConfig: true,
        basedOnRevision: view.revision,
      } as never);
    },
  ],
  [
    "draft: switch to manual",
    async (a) => {
      const view = await a.getView();
      return a.savePersonalModelDraft({
        providerId: "p-openai",
        originalModelId: "renamed",
        nextModelId: "renamed",
        personalConfig: manualConfig,
        useRecommendedConfig: false,
        basedOnRevision: view.revision,
      } as never);
    },
  ],
  [
    "draft: stale registry revision",
    (a) =>
      a.savePersonalModelDraft({
        providerId: "p-openai",
        originalModelId: "renamed",
        nextModelId: "renamed",
        personalConfig: {},
        basedOnRevision: 0,
      } as never),
    { anyMessage: true },
  ],
  [
    "draft: rename inherited model",
    async (a) => {
      const view = await a.getView();
      return a.savePersonalModelDraft({
        providerId: "p-openai",
        originalModelId: "gpt-5.6",
        nextModelId: "zzz",
        personalConfig: {},
        basedOnRevision: view.revision,
      } as never);
    },
  ],
  [
    "draft: edit inherited model config",
    async (a) => {
      const view = await a.getView();
      return a.savePersonalModelDraft({
        providerId: "p-openai",
        originalModelId: "gpt-5.4",
        nextModelId: "gpt-5.4",
        personalConfig: { enabled: false },
        basedOnRevision: view.revision,
      } as never);
    },
  ],
  [
    "draft: unknown original",
    async (a) => {
      const view = await a.getView();
      return a.savePersonalModelDraft({
        providerId: "p-openai",
        originalModelId: "nope",
        nextModelId: "nope",
        personalConfig: {},
        basedOnRevision: view.revision,
      } as never);
    },
  ],
  [
    "resolve: inherited model",
    (a) => a.resolveModelConfig({ providerId: "p-openai", modelId: "gpt-5.6" } as never),
  ],
  [
    "resolve: manual model",
    (a) => a.resolveModelConfig({ providerId: "p-openai", modelId: "my-model" } as never),
  ],
  [
    "resolve: personal model without rules",
    (a) => a.resolveModelConfig({ providerId: "p-custom", modelId: "c1-renamed" } as never),
  ],
  [
    "resolve: account provider",
    (a) =>
      a.resolveModelConfig({ providerId: "account:zai-start-plan", modelId: "GLM-5.2" } as never),
  ],
  [
    "resolve: unknown provider",
    (a) => a.resolveModelConfig({ providerId: "ghost", modelId: "x" } as never),
  ],
  [
    "resolve: draft preview",
    (a) =>
      a.resolveModelConfig({
        providerId: "p-openai",
        originalModelId: "my-model",
        modelId: "my-model",
        personalConfig: { enabled: false, properties: { contextWindow: 1234 } },
      } as never),
  ],
  [
    "resolve: draft preview with rename",
    (a) =>
      a.resolveModelConfig({
        providerId: "p-openai",
        originalModelId: "my-model",
        modelId: "renamed-preview",
        personalConfig: {},
      } as never),
  ],
  [
    "resolve: invalid draft",
    (a) =>
      a.resolveModelConfig({
        providerId: "p-openai",
        originalModelId: "my-model",
        modelId: "my-model",
        personalConfig: { surprise: true },
      } as never),
    { anyMessage: true },
  ],
  [
    "connectivity: unknown provider",
    (a) =>
      a.testModelConnectivity({ workspacePath: "/w", providerId: "ghost", modelId: "x" } as never),
  ],
  [
    "connectivity: unknown model",
    (a) =>
      a.testModelConnectivity({
        workspacePath: "/w",
        providerId: "p-openai",
        modelId: "nope",
      } as never),
  ],
  [
    "connectivity: disabled personal model",
    (a) =>
      a.testModelConnectivity({
        workspacePath: "/w",
        providerId: "p-openai",
        modelId: "my-model",
      } as never),
  ],
  ["delete provider", (a) => a.deletePersonalProvider("p-custom")],
  ["delete missing provider", (a) => a.deletePersonalProvider("ghost")],
  ["delete overlay of builtin provider", (a) => a.deletePersonalProvider("account:zai-start-plan")],
];

const strip = (value: unknown) => {
  const copy = JSON.parse(JSON.stringify(value ?? null)) as Record<string, unknown>;
  const clean = (item: unknown) => {
    if (item && typeof item === "object" && "revision" in item) {
      (item as Record<string, unknown>).revision = "<n>";
    }
  };
  clean(copy);
  clean(copy?.view);
  return copy;
};

async function outcome(run: () => Promise<unknown>) {
  try {
    return { ok: true as const, value: strip(await run()) };
  } catch (error) {
    return { ok: false as const, message: error instanceof Error ? error.message : String(error) };
  }
}

const DIRTY = {
  schemaVersion: 1,
  config: {
    providerConfigRules: {
      providerRules: [
        {
          providerId: "p-dirty",
          templateId: "openai",
          config: {
            group: "standard-personal",
            access: key,
            personalModelIds: ["gpt-5.6", "mine", "mine", "other"],
            modelOrder: ["other", "ghost", "gpt-5.4"],
          },
        },
      ],
    },
    modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] },
  },
};

const FRESH_OPS: Op[] = [
  ["fresh: create bare", (a) => a.createPersonalProvider()],
  ["fresh: create template", (a) => a.createPersonalProvider({ templateId: "OpenAI " })],
  ["fresh: add model", (a) => a.addPersonalModel("openai", "mine", {} as never)],
  [
    "fresh: toggle inherited",
    (a) => a.setPersonalModelEnabled("openai", "gpt-5.4", false as never),
  ],
  ["fresh: reorder providers", (a) => a.reorderPersonalProviders(["openai", "new-provider"])],
  ["fresh: delete", (a) => a.deletePersonalProvider("new-provider")],
];

const DIRTY_OPS: Op[] = [
  [
    "dirty: save overlay normalizes membership",
    (a) =>
      a.savePersonalProviderOverlay("p-dirty", {
        group: "standard-personal",
        access: key,
      } as never),
  ],
  ["dirty: add model", (a) => a.addPersonalModel("p-dirty", "extra", {} as never)],
  ["dirty: delete model", (a) => a.deletePersonalModel("p-dirty", "other")],
  ["dirty: reorder", (a) => a.reorderPersonalModels("p-dirty", ["extra", "mine"])],
];

const SCENARIOS: Array<[string, unknown, Op[]]> = [
  ["populated", INITIAL, OPS],
  ["no personal file", null, FRESH_OPS],
  ["dirty membership and absent order", DIRTY, DIRTY_OPS],
];

const normalize = (value: unknown, root: string): unknown =>
  JSON.parse(JSON.stringify(value).split(root).join("$FIXTURE_ROOT"));

const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-provider-w-golden-")));
const builtin = join(base, "escode-builtin.json");
const tsPersonal = join(base, "ts_provider_config.json");
await copyFile(BUILTIN_SOURCE, builtin);

const scenarios: Array<Record<string, unknown>> = [];
try {
  for (const [name, initial, ops] of SCENARIOS) {
    await rm(tsPersonal, { force: true });
    if (initial !== null) await writeFile(tsPersonal, JSON.stringify(initial, null, 2));
    const runtime = createProviderRuntime({
      escodeBuiltinFilePath: builtin,
      personalFilePath: tsPersonal,
      watch: false,
      personalPollingIntervalMs: false,
      testConnectivity: async () => ({ success: true }),
    });
    const ts = runtime.providerSettings as unknown as Api;
    const steps: Array<Record<string, unknown>> = [];
    try {
      await ts.getView();
      for (const [label, op, options] of ops) {
        const expected = await outcome(() => op(ts));
        steps.push({
          label,
          ok: expected.ok,
          anyMessage: Boolean(options?.anyMessage),
          ...(expected.ok
            ? { value: expected.value }
            : options?.anyMessage
              ? {}
              : { message: expected.message }),
          file: normalize(JSON.parse(await readFile(tsPersonal, "utf8")) as unknown, base),
        });
      }
    } finally {
      runtime.dispose();
    }
    scenarios.push({ label: name, ops: steps });
  }
} finally {
  await rm(base, { recursive: true, force: true });
}

const golden = {
  _readme:
    "rustHostProviderWrite.test.ts 的 TS createProviderRuntime.providerSettings 写操作观察冻结（Phase 3 golden freeze）。revision 归一化为 <n>；anyMessage 用例不冻结文案；绝对临时路径归一化为 $FIXTURE_ROOT。",
  scenarios,
};
const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenPath = join(goldenDir, "rustHostProviderWrite.oracle.json.gz");
const json = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(gunzipSync(await readFile(goldenPath)).toString("utf8")),
    JSON.parse(json),
    "rustHostProviderWrite golden 与当前 TS 观察漂移",
  );
  console.log("rustHostProviderWrite golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, gzipSync(Buffer.from(json)));
  console.log("wrote", goldenPath);
}
