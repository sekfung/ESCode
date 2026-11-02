// Run with node --import tsx. MCP 工具卡 `_meta.ui` 投影的 TS oracle 语料
// （docs/specs/rust-mcp-ui.md §5.5）：`normalizeMcpToolUiMeta` 描述符归一化与
// `createMcpToolDisplay` 的行级 `display.ui`。--check 防漂移。
//
// 语料约束（避免把既有实现差异混进本差分）：
// - JSON 载荷的键一律按字典序书写：Rust `serde_json::to_string` 默认排序键，JS `JSON.stringify`
//   保持插入序；键序不同不影响语义，本条差分只对齐文本内容。
// - 只用整数与字符串值：JS 把 `1.0` 序列化成 `1`、`-0` 序列化成 `0`，与 serde_json 不一致。
// - 资源 URI / surface / pluginId 只用 ASCII：TS 按 UTF-16 码元计长，Rust 按字符计长。
import { readFile, writeFile } from "node:fs/promises";
import { normalizeMcpToolUiMeta } from "../packages/shared/src/mcp-apps/toolMeta.ts";
import { createMcpToolDisplay } from "../apps/escode-cli/packages/core/src/tool/executor/result-display.ts";
import {
  MCP_APPS_CONTENT_MAX_BYTES,
  MCP_APPS_RESOURCE_URI_MAX_CHARS,
  MCP_APPS_STRUCTURED_CONTENT_MAX_BYTES,
  MCP_APPS_SURFACE_ID_MAX_CHARS,
  MCP_APPS_WIDGET_META_MAX_BYTES,
} from "../packages/shared/src/mcp-apps/contract.ts";

const base = (extra) => ({
  ui: { resourceUri: "ui://server/index.html", ...extra.ui },
  ...extra.rest,
});

const metas = [
  ["no-meta", {}],
  ["null-meta", null],
  ["non-object-meta", "ui://server"],
  ["ui-array-meta", { ui: [] }],
  ["spec-resource-uri", { ui: { resourceUri: "ui://server/index.html" } }],
  ["trimmed-resource-uri", { ui: { resourceUri: "  ui://server/padded.html\t" } }],
  ["legacy-flat-resource-uri", { "ui/resourceUri": "ui://legacy/index.html" }],
  ["openai-output-template", { "openai/outputTemplate": "ui://alias/index.html" }],
  [
    "resource-uri-priority",
    {
      ui: { resourceUri: "ui://spec/index.html" },
      "ui/resourceUri": "ui://legacy/index.html",
      "openai/outputTemplate": "ui://alias/index.html",
    },
  ],
  [
    "invalid-spec-falls-back-to-legacy",
    { ui: { resourceUri: "https://example.com/index.html" }, "ui/resourceUri": "ui://legacy" },
  ],
  [
    "invalid-all-sources",
    {
      ui: { resourceUri: "" },
      "ui/resourceUri": "ui://",
      "openai/outputTemplate": 42,
    },
  ],
  [
    "resource-uri-too-long",
    { ui: { resourceUri: "ui://" + "a".repeat(MCP_APPS_RESOURCE_URI_MAX_CHARS) } },
  ],
  [
    "resource-uri-max-length",
    {
      ui: {
        resourceUri: "ui://" + "a".repeat(MCP_APPS_RESOURCE_URI_MAX_CHARS - "ui://".length),
      },
    },
  ],
  ["visibility-app-only", base({ ui: { visibility: ["app"] } })],
  ["visibility-dedup-keeps-order", base({ ui: { visibility: ["app", "model", "app"] } })],
  ["visibility-unknown-only", base({ ui: { visibility: ["bogus"] } })],
  ["visibility-non-array", base({ ui: { visibility: "model" } })],
  ["visibility-mixed", base({ ui: { visibility: [1, "model", true, "app"] } })],
  ["display-mode-inline", { ...base({}), "openai/ui": { preferredModelDisplayMode: "inline" } }],
  [
    "display-mode-fullscreen",
    { ...base({}), "openai/ui": { preferredModelDisplayMode: "fullscreen" } },
  ],
  ["display-mode-pip", { ...base({}), "openai/ui": { preferredModelDisplayMode: "pip" } }],
  ["display-mode-invalid", { ...base({}), "openai/ui": { preferredModelDisplayMode: "INLINE" } }],
  [
    "csp-modern",
    base({
      ui: {
        csp: {
          connectDomains: ["https://api.example.com", "wss://socket.example.com"],
          resourceDomains: ["https://cdn.example.com:8443"],
        },
      },
    }),
  ],
  [
    "csp-modern-dedup-and-invalid",
    base({
      ui: {
        csp: {
          connectDomains: [
            "https://api.example.com/",
            "https://api.example.com",
            "http://127.0.0.1:7777",
            "http://localhost:7777",
            "https://user:pass@example.com",
            "https://example.com?query=1",
            "https://example.com#hash",
            "https://example.com/path",
            "ftp://example.com",
            "not a url",
            42,
          ],
        },
      },
    }),
  ],
  [
    "csp-legacy-camel-and-snake",
    {
      ...base({}),
      "openai/widgetCSP": {
        connectDomains: ["https://camel.example.com"],
        resource_domains: ["https://snake.example.com"],
        frameDomains: [],
      },
    },
  ],
  [
    "csp-modern-wins-over-legacy",
    {
      ...base({ ui: { csp: { connectDomains: ["https://modern.example.com"] } } }),
      "openai/widgetCSP": { connectDomains: ["https://legacy.example.com"] },
    },
  ],
  ["csp-unknown-keys-only", { ...base({}), "openai/widgetCSP": { scriptDomains: ["https://x"] } }],
  ["prefers-border-true", base({ ui: { prefersBorder: true } })],
  ["prefers-border-false", base({ ui: { prefersBorder: false } })],
  ["prefers-border-legacy", { ...base({}), "openai/widgetPrefersBorder": true }],
  [
    "prefers-border-modern-false-wins",
    { ...base({ ui: { prefersBorder: false } }), "openai/widgetPrefersBorder": true },
  ],
  ["surface-valid", base({ ui: { surface: "board-1.2_3" } })],
  ["surface-trimmed", base({ ui: { surface: "  board  " } })],
  ["surface-leading-symbol", base({ ui: { surface: "-board" } })],
  ["surface-too-long", base({ ui: { surface: "b".repeat(MCP_APPS_SURFACE_ID_MAX_CHARS + 1) } })],
  ["surface-invalid-char", base({ ui: { surface: "board/1" } })],
  ["show-inline-true", base({ ui: { showInline: true } })],
  ["show-inline-false", base({ ui: { showInline: false } })],
  ["show-inline-legacy", { ...base({}), "openai/widgetShowCodexWidgetInline": true }],
  [
    "full-descriptor",
    {
      ui: {
        resourceUri: "ui://server/app.html",
        visibility: ["app"],
        prefersBorder: true,
        surface: "panel",
        showInline: true,
        csp: { connectDomains: ["https://api.example.com"] },
      },
      "openai/ui": { preferredModelDisplayMode: "fullscreen" },
    },
  ],
].map(([label, meta]) => ({ label, meta, descriptor: normalizeMcpToolUiMeta(meta) }));

const structuredWithin = { html: "<p>ok</p>", title: "Board" };
const widgetMetaWithin = { theme: "dark" };
const contentWithin = [{ text: "hello", type: "text" }];

// 超预算载荷不写进 JSON：按 `repeat` 计划在两边本地重建，避免把 240 KiB 的填充字符串塞进仓库。
// 期望值仍由 TS `createMcpToolDisplay` 给出，含 truncatedBytes 的累加结果。
const repeatText = (char, times) => char.repeat(times);
const buildRepeats = (specs) => {
  const output = {};
  for (const spec of specs) {
    const text = repeatText(spec.char, spec.times);
    if (spec.field === "content") output.content = [{ text, type: "text" }];
    else output[spec.field] = text;
  }
  return output;
};
const structuredOver = [
  { field: "structuredContent", char: "a", times: MCP_APPS_STRUCTURED_CONTENT_MAX_BYTES },
];
const widgetMetaOver = [{ field: "_meta", char: "b", times: MCP_APPS_WIDGET_META_MAX_BYTES }];
const contentOver = [{ field: "content", char: "c", times: MCP_APPS_CONTENT_MAX_BYTES }];

const displayCases = [
  ["no-ui-meta", { meta: {}, serverName: "plain", pluginId: "plain", output: undefined }],
  ["descriptor-only", { meta: { ui: { resourceUri: "ui://server/index.html" } } }],
  [
    "structured-content-within-budget",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      output: { structuredContent: structuredWithin },
    },
  ],
  [
    "structured-content-over-budget",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      repeat: structuredOver,
    },
  ],
  [
    "widget-meta-over-budget",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      repeat: widgetMetaOver,
    },
  ],
  [
    "widget-meta-within-budget",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      output: { _meta: widgetMetaWithin },
    },
  ],
  [
    "content-array-within-budget",
    { meta: { ui: { resourceUri: "ui://server/index.html" } }, output: { content: contentWithin } },
  ],
  [
    "content-non-array-is-skipped",
    { meta: { ui: { resourceUri: "ui://server/index.html" } }, output: { content: { text: "x" } } },
  ],
  [
    "content-over-budget",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      repeat: contentOver,
    },
  ],
  [
    "two-fields-truncated-add-up",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      repeat: [...structuredOver, ...contentOver],
    },
  ],
  [
    "is-error-true",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      output: { isError: true, structuredContent: { code: 7 } },
    },
  ],
  [
    "is-error-false",
    { meta: { ui: { resourceUri: "ui://server/index.html" } }, output: { isError: false } },
  ],
  [
    "non-object-output",
    { meta: { ui: { resourceUri: "ui://server/index.html" } }, output: "text output" },
  ],
  [
    "surface-and-show-inline-only-when-true",
    {
      meta: { ui: { resourceUri: "ui://server/index.html", surface: "panel", showInline: false } },
      output: { structuredContent: { rows: 1 } },
    },
  ],
  [
    "surface-and-show-inline-projected",
    {
      meta: { ui: { resourceUri: "ui://server/index.html", surface: "panel", showInline: true } },
    },
  ],
  [
    "preferred-display-mode-and-border",
    {
      meta: {
        ui: { resourceUri: "ui://server/index.html", prefersBorder: false },
        "openai/ui": { preferredModelDisplayMode: "pip" },
      },
    },
  ],
  [
    "csp-projected",
    {
      meta: {
        ui: {
          resourceUri: "ui://server/index.html",
          csp: { connectDomains: ["https://api.example.com"] },
        },
      },
    },
  ],
  [
    "blank-plugin-id-drops-ui",
    { meta: { ui: { resourceUri: "ui://server/index.html" } }, pluginId: "   " },
  ],
  [
    "namespaced-plugin-server",
    {
      meta: { ui: { resourceUri: "ui://server/index.html" } },
      serverName: "plugin:acme:srv",
      pluginId: "acme",
      pluginIdSource: "server",
    },
  ],
  [
    "long-plugin-id-truncated",
    { meta: { ui: { resourceUri: "ui://server/index.html" } }, pluginId: "p".repeat(257) },
  ],
].map(([label, overrides]) => {
  const serverName = overrides.serverName ?? "server";
  const pluginId = overrides.pluginId ?? serverName;
  const output = overrides.repeat ? buildRepeats(overrides.repeat) : overrides.output;
  const display = createMcpToolDisplay(
    {
      serverName,
      toolName: "tool",
      pluginId,
      ui: normalizeMcpToolUiMeta(overrides.meta),
    },
    output,
  );
  return {
    label,
    meta: overrides.meta,
    serverName,
    // 语料里 pluginId 是 TS catalog 给出的值；`pluginIdSource: "server"` 的用例要求 Rust
    // `plugin_id_for_server` 从同一个 server 名解析出相同结果。
    pluginId,
    ...(overrides.pluginIdSource ? { pluginIdSource: overrides.pluginIdSource } : {}),
    ...(overrides.repeat ? { repeat: overrides.repeat } : { output: output ?? null }),
    display: display ?? null,
  };
});

const content = `${JSON.stringify(
  {
    constants: {
      contentMaxBytes: MCP_APPS_CONTENT_MAX_BYTES,
      maxNameChars: 256,
      resourceUriMaxChars: MCP_APPS_RESOURCE_URI_MAX_CHARS,
      structuredContentMaxBytes: MCP_APPS_STRUCTURED_CONTENT_MAX_BYTES,
      surfaceIdMaxChars: MCP_APPS_SURFACE_ID_MAX_CHARS,
      widgetMetaMaxBytes: MCP_APPS_WIDGET_META_MAX_BYTES,
    },
    meta: metas,
    displayUi: displayCases,
  },
  null,
  1,
)}\n`;
const path = new URL(
  "../apps/escode-cli-rust/crates/domain/tests/fixtures/mcp_tool_ui_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  const current = await readFile(path, "utf8").catch(() => "");
  if (current !== content) throw new Error(`Rust MCP tool UI corpus differs from TS: ${path}`);
} else await writeFile(path, content);
