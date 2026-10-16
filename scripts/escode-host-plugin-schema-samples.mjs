// 有效样本（真实 zod schema + 一份手写的有效值）。变异与求值在 generate-escode-host-plugin-schema-fixtures.mjs。

import {
  escodePluginsCancelOperationResultSchema,
  escodePluginsConfigureResultSchema,
  escodePluginsDescribeResultSchema,
  escodePluginsInstallResultSchema,
  escodePluginsListResultSchema,
  escodePluginsMarketplaceMutationResultSchema,
  escodePluginsOverviewResultSchema,
  escodePluginsReferenceCatalogResultSchema,
  escodePluginsResolveSuggestedReferenceResultSchema,
  escodePluginsRestoreBuiltinResultSchema,
  escodePluginsSetEnabledResultSchema,
  escodePluginsUninstallResultSchema,
  escodePluginsValidateResultSchema,
} from "../packages/shared/src/escode-protocol/index.ts";

const hook = {
  event: "PreToolUse",
  matcher: "Bash",
  type: "command",
  command: "echo",
  args: ["a"],
  async: false,
  shell: true,
  timeout: 1.5,
  timeoutMs: 2000,
  statusMessage: "running",
  sourcePath: "/p/hooks.json",
  runnable: true,
};
const listing = {
  displayName: "Demo",
  displayNameI18n: { zh: "演示" },
  descriptionI18n: { zh: "描述" },
  icon: "i",
  category: "c",
  author: "a",
  authorUrl: "https://a.example",
  homepage: "https://h.example",
  privacyPolicy: "p",
  termsOfService: "t",
  heroImage: "h",
  examplePrompts: ["p1"],
  examplePromptsI18n: { zh: ["提示"] },
  requiresPaidPlan: true,
};
const info = {
  id: "demo@mk",
  name: "demo",
  description: "d",
  version: "1.0.0",
  enabled: true,
  source: "mk",
  marketplace: "mk",
  author: "a",
  authorUrl: "https://a.example",
  homepage: "https://h.example",
  skillCount: 2,
  skillRootCount: 1,
  commandRootCount: 0,
  components: [{ kind: "skill", items: [{ name: "s", description: "sd" }] }],
  declaredMcpServerNames: ["m"],
  hostMcpServerNames: ["m"],
  mcpServerNames: ["m"],
  hookDetails: [hook],
  rootPath: "/p",
  userConfig: {
    key: {
      default: 1,
      description: "d",
      required: true,
      sensitive: false,
      title: "t",
      type: "number",
    },
  },
  configuredOptions: { key: "v", n: 2, b: false },
  packageStatus: "missing",
  rootSource: "user",
  enabledSource: "workspace",
  optionSources: { key: "user" },
};
const diagnostic = { code: "c", message: "m", severity: "warning", pluginId: "demo@mk" };
const available = {
  id: "demo@mk",
  name: "demo",
  marketplace: "mk",
  description: "d",
  version: "1",
  installed: false,
  componentTypes: ["skill"],
  listing,
};
const installed = {
  id: "demo@mk",
  name: "demo",
  marketplace: "mk",
  description: "d",
  version: "1",
  enabled: true,
  scope: "user",
  installPath: "/i",
  installedAt: "2026-01-01",
  componentTypes: ["skill"],
  hookDetails: [hook],
  updateStatus: "update-available",
  latestVersion: "2",
  listing,
};
const marketplace = {
  id: "mk",
  name: "mk",
  source: { type: "github", repo: "a/b" },
  description: "d",
  lastUpdated: "2026-01-01",
  pluginCount: 3,
  isOfficial: true,
  featured: ["demo"],
  refreshFailure: { code: "c", failedAt: "2026-01-01", message: "m" },
};

const catalogEntry = {
  category: "dev",
  pluginId: "demo@mk",
  name: "demo",
  marketplace: "mk",
  icon: "i",
  displayName: "Demo",
  displayNameI18n: { zh: "演示" },
  description: "d",
  descriptionI18n: { zh: "描述" },
  enabled: true,
  conflictingPluginIds: ["x@mk"],
  skillQualifiedNames: ["demo:s"],
  mcpServerNames: ["m"],
  subagentNames: ["sub"],
};

export const SAMPLES = [
  {
    schema: "plugins/list",
    zod: escodePluginsListResultSchema,
    values: [
      { plugins: [info], diagnostics: [diagnostic] },
      { plugins: [], diagnostics: [] },
      {
        plugins: [
          {
            id: "a",
            name: "a",
            enabled: false,
            source: "s",
            marketplace: "m",
            skillRootCount: 0,
            commandRootCount: 0,
            mcpServerNames: [],
            rootPath: "",
          },
        ],
        diagnostics: [{ code: "", message: "" }],
      },
    ],
  },
  {
    schema: "plugins/overview",
    zod: escodePluginsOverviewResultSchema,
    values: [
      {
        marketplaces: [marketplace],
        availablePlugins: [available],
        installedPlugins: [installed],
        restorableBuiltins: [available],
        diagnostics: [diagnostic],
        capability: { supported: true, reason: "r" },
      },
      {
        marketplaces: [],
        availablePlugins: [],
        installedPlugins: [],
        restorableBuiltins: [],
        diagnostics: [],
        capability: { supported: false },
      },
    ],
  },
  {
    schema: "plugins/marketplace/add",
    zod: escodePluginsMarketplaceMutationResultSchema,
    values: [{ marketplace, marketplaces: [marketplace], diagnostics: [diagnostic] }, {}],
  },
  {
    schema: "plugins/install",
    zod: escodePluginsInstallResultSchema,
    values: [
      { installedPlugins: [installed], dependencyClosure: ["dep@mk"], diagnostics: [diagnostic] },
      { installedPlugins: [], dependencyClosure: [], diagnostics: [] },
    ],
  },
  {
    schema: "plugins/cancelOperation",
    zod: escodePluginsCancelOperationResultSchema,
    values: [{ operationId: "op-1", cancelled: true }],
  },
  {
    schema: "plugins/uninstall",
    zod: escodePluginsUninstallResultSchema,
    values: [{ removedPlugin: installed, diagnostics: [diagnostic] }, { diagnostics: [] }],
  },
  {
    schema: "plugins/restoreBuiltin",
    zod: escodePluginsRestoreBuiltinResultSchema,
    values: [{ pluginId: "demo@mk", diagnostics: [diagnostic] }],
  },
  {
    schema: "plugins/configure",
    zod: escodePluginsConfigureResultSchema,
    values: [{ pluginId: "demo@mk", diagnostics: [] }],
  },
  {
    schema: "plugins/validate",
    zod: escodePluginsValidateResultSchema,
    values: [
      {
        ok: true,
        diagnostics: [diagnostic],
        compatibility: { runnable: ["a"], diagnosticOnly: ["b"], unsupported: ["c"] },
      },
    ],
  },
  {
    schema: "plugins/describe",
    zod: escodePluginsDescribeResultSchema,
    values: [
      {
        components: [{ kind: "mcp", items: [{ name: "m", description: "d" }] }],
        diagnostics: [diagnostic],
        metadata: { author: "a", authorUrl: "u", homepage: "h", version: "1" },
      },
      { components: [] },
    ],
  },
  {
    schema: "plugins/setEnabled",
    zod: escodePluginsSetEnabledResultSchema,
    values: [{ plugin: info, enabled: true }],
  },
  {
    schema: "plugins/resolveSuggestedReference",
    zod: escodePluginsResolveSuggestedReferenceResultSchema,
    values: [
      {
        stableId: "official:demo",
        status: "ready",
        marketplace: "mk",
        pluginName: "demo",
        sourceTrust: "official",
        icon: "i",
        listing,
        diagnostics: [diagnostic],
      },
      { stableId: "official:demo", status: "conflict", diagnostics: [] },
      { stableId: "official:demo", status: "unavailable", icon: "i", diagnostics: [] },
    ],
  },
  {
    schema: "plugins/referenceCatalogWithCategory",
    zod: escodePluginsReferenceCatalogResultSchema,
    values: [
      { authority: "session", plugins: [catalogEntry] },
      {
        authority: "workspace",
        plugins: [
          {
            pluginId: "a@mk",
            name: "a",
            marketplace: "mk",
            enabled: false,
            conflictingPluginIds: [],
            skillQualifiedNames: [],
            mcpServerNames: [],
          },
        ],
      },
    ],
  },
];

// 唯一带 `.default(...)` 的 schema：`parse` 的输出不等于输入，需要单独导出归一化结果。
export const NORMALIZED_SCHEMAS = new Set(["plugins/referenceCatalogWithCategory"]);
