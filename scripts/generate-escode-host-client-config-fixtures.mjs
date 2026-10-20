// Run with node --import tsx. 用**真实 TS** `parseClientConfigSnapshot`（packages/shared/src/clientConfig.ts）对一组信封求值，
// 供 Rust 侧 `client_config::snapshot` 对拍（docs/specs/rust-u2-client-config.md §5 S2）。
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { parseClientConfigSnapshot } from "../packages/shared/src/clientConfig.ts";
import {
  ESCODE_DYNAMIC_WORKFLOW_MODE_ENV,
  resolveDynamicWorkflowClientConfig,
} from "../packages/shared/src/dynamic-workflow-feature.ts";

const FIXTURE_VERSION = 1;
const OUT = "apps/escode-host/crates/host-services/src/client_config/snapshot_golden.json";

const ABSENT = Symbol("absent");

const ENVELOPES = [
  null,
  42,
  "x",
  true,
  [],
  {},
  { code: 1 },
  { code: "0" },
  { code: null },
  { code: 0 },
  { code: 0, data: null },
  { code: 0, data: [] },
  { code: 0, data: "x" },
  { code: 0, data: 1 },
  { code: 0, data: {} },
  { code: 0, data: { configs: null } },
  { code: 0, data: { configs: [] } },
  { code: 0, data: { configs: "x" } },
  { code: 0, data: { configs: {} } },
  { code: 0, data: { configs: {}, extra: 1 }, extra: 2 },
  { code: 0, msg: "ok", data: { configs: { other: 1 } } },
];

const ZSRC = [
  ABSENT,
  null,
  "https://a.example.com",
  [],
  {},
  { url: 1 },
  { url: null },
  { url: "" },
  { url: "   " },
  { url: "https://a.example.com" },
  { url: "  https://a.example.com/x  " },
  { url: "HTTPS://A.EXAMPLE.COM" },
  { url: "http://a.example.com" },
  { url: "ftp://a.example.com" },
  { url: "http://" },
  { url: "https://" },
  { url: "https://a b.example.com" },
  { url: "javascript:alert(1)" },
  { url: "http:foo" },
  { url: "https:foo" },
  { url: "https://user:pw@a.example.com:8080/p?q=1#h" },
  { url: "//a.example.com" },
  { url: " https://a.example.com " },
  { url: "﻿https://a.example.com" },
  { url: "\u0085https://a.example.com" },
  { url: "https://a.example.com", extra: 1 },
  { url: "http://[::1]/" },
  { url: "http://localhost" },
  { url: "https://münchen.de/ü" },
  { url: "http://a.example.com:99999" },
  { url: "http://a.example.com:80" },
  { url: "https://a.example.com/a b" },
  { url: "https://exa_mple.com" },
  { url: "http://256.256.256.256" },
  { url: "httpx://a.example.com" },
  { url: "https://a.example.com\n" },
];

const ORDER = [
  ABSENT,
  null,
  [],
  "x",
  7,
  {},
  { code: {} },
  { code: null },
  { code: [] },
  { code: "x" },
  { code: { categoryOrder: [] } },
  { code: { categoryOrder: ["a"] } },
  { code: { categoryOrder: ["a", " b", "a", "b "] } },
  { code: { categoryOrder: ["a", ""] } },
  { code: { categoryOrder: ["a", "  "] } },
  { code: { categoryOrder: ["a", 1] } },
  { code: { categoryOrder: "a" } },
  { code: { categoryOrder: null } },
  { code: { pluginOrder: {} } },
  { code: { pluginOrder: { x: ["a", "a"], y: [] } } },
  { code: { pluginOrder: { x: ["a", ""] } } },
  { code: { pluginOrder: { x: "a" } } },
  { code: { pluginOrder: [] } },
  { code: { pluginOrder: null } },
  { code: { categoryOrder: ["a"], pluginOrder: { x: ["b"] }, extra: 1 }, extra: 2 },
  { code: { categoryOrder: ["a", 1] }, work: { categoryOrder: ["z"] } },
  { code: { categoryOrder: ["a"] }, work: "bad" },
  { code: { categoryOrder: ["a"] }, work: { pluginOrder: { p: ["q", "q ", " q"] } } },
  { work: { categoryOrder: [" a ", "﻿b", "\u0085c", "​d"] } },
  { code: { categoryOrder: ["日本語", " 日本語 "] } },
  {
    code: { categoryOrder: ["a"] },
    work: { categoryOrder: ["a"] },
    other: { categoryOrder: ["x"] },
  },
];

// 动态工作流灰度：远端值 × 环境覆盖 × 用户模式的全组合（纯函数 `resolveDynamicWorkflowClientConfig`）。
const REMOTE = [
  ABSENT,
  null,
  "x",
  [],
  {},
  { mode: "disabled" },
  { mode: "onDemand" },
  { mode: "alwaysOn" },
  { mode: " onDemand " },
  { mode: "bogus" },
  { mode: 1 },
  { mode: null },
  { mode: "ONDEMAND" },
  { mode: "\u00a0alwaysOn" },
  { mode: "\u0085alwaysOn" },
  { mode: "alwaysOn", extra: 1 },
];
const ENV_OVERRIDE = [
  ABSENT,
  "",
  "disabled",
  "onDemand",
  "alwaysOn",
  " alwaysOn ",
  "bogus",
  "\ufeffonDemand",
];
const USER_MODE = [
  ABSENT,
  null,
  "disabled",
  "onDemand",
  "alwaysOn",
  " onDemand ",
  "bogus",
  1,
  {},
  [],
  "\u0085disabled",
];

function dynamicWorkflowCases() {
  const cases = [];
  for (const remote of REMOTE) {
    for (const override of ENV_OVERRIDE) {
      for (const userMode of USER_MODE) {
        const result = resolveDynamicWorkflowClientConfig({
          remote: remote === ABSENT ? undefined : remote,
          env: override === ABSENT ? {} : { [ESCODE_DYNAMIC_WORKFLOW_MODE_ENV]: override },
          userMode: userMode === ABSENT ? undefined : userMode,
        });
        // 缺省 = 键不存在（Rust 侧据此还原 `undefined`）。
        const wire = {};
        if (remote !== ABSENT) wire.remote = remote;
        if (override !== ABSENT) wire.env = override;
        if (userMode !== ABSENT) wire.userMode = userMode;
        cases.push({ input: wire, expected: JSON.parse(JSON.stringify(result)) });
      }
    }
  }
  return cases;
}

function envelope(configs) {
  const entries = {};
  if (configs.zsrc !== ABSENT) entries.zsrc = configs.zsrc;
  if (configs.order !== ABSENT) entries.pluginStoreOrder = configs.order;
  return { code: 0, data: { configs: entries } };
}

function evaluate(payload) {
  try {
    return { ok: true, snapshot: parseClientConfigSnapshot(payload) };
  } catch (error) {
    return { ok: false, message: error instanceof Error ? error.message : String(error) };
  }
}

function build() {
  const payloads = [...ENVELOPES];
  for (const zsrc of ZSRC) payloads.push(envelope({ zsrc, order: ABSENT }));
  for (const order of ORDER) payloads.push(envelope({ zsrc: ABSENT, order }));
  // 两个字段互不影响：交叉少量组合。
  for (const zsrc of [ZSRC[2], ZSRC[9], ZSRC[13]]) {
    for (const order of [ORDER[11], ORDER[15], ORDER[26]]) payloads.push(envelope({ zsrc, order }));
  }
  const cases = payloads.map((payload) => {
    // JSON 往返，保证 Rust 看到的就是 TS 求值时的值（`undefined` 键消失）。
    const wire = JSON.parse(JSON.stringify(payload));
    const result = evaluate(wire);
    return { payload: wire, expected: JSON.parse(JSON.stringify(result)) };
  });
  const accepted = cases.filter((entry) => entry.expected.ok).length;
  if (accepted === 0 || accepted === cases.length) {
    throw new Error("corpus must contain both accepted and rejected payloads");
  }
  return { fixtureVersion: FIXTURE_VERSION, cases, dynamicWorkflow: dynamicWorkflowCases() };
}

async function main() {
  const payload = build();
  const text = `${JSON.stringify(payload)}\n`;
  if (process.argv.includes("--check")) {
    const current = await readFile(OUT, "utf8").catch(() => null);
    if (current !== text) {
      console.error(
        `[client-config] 期望资产已过期：${OUT}\n运行 node --import tsx ${process.argv[1]} 重新生成`,
      );
      process.exit(1);
    }
    console.log(`[client-config] 期望资产一致：${OUT}`);
    return;
  }
  await mkdir(dirname(OUT), { recursive: true });
  await writeFile(OUT, text);
  process.stdout.write(`client-config golden: ${payload.cases.length} cases → ${OUT}\n`);
}

await main();
