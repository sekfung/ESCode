// Phase 3 golden freeze：rustHostDynamicWorkflow.test.ts 的 TS `createClientConfigService`
// `getDynamicWorkflowClientConfig` 观察录制。一次性脚本；删除 TS 实现前用于重录。
// `--check` 校验仓内 golden 与当前 TS 观察一致。
import { deepEqual } from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { join } from "node:path";
import { ESCODE_DYNAMIC_WORKFLOW_MODE_ENV, ESCODE_VERSION } from "../packages/shared/src/index.ts";
import { createClientConfigService, createNodeApiClient } from "../packages/services/src/node.ts";

interface Stub {
  origin: string;
  requests: () => number;
  reply: (status: number, body: unknown) => void;
  close: () => Promise<void>;
}

async function startStub(): Promise<Stub> {
  let count = 0;
  let current = { status: 200, body: "{}" };
  const server = createServer((_request, response) => {
    count += 1;
    response.statusCode = current.status;
    response.setHeader("content-type", "application/json");
    response.end(current.body);
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  const { port } = server.address() as AddressInfo;
  return {
    origin: `http://127.0.0.1:${port}`,
    requests: () => count,
    reply: (status, body) => {
      current = { status, body: typeof body === "string" ? body : JSON.stringify(body) };
    },
    close: () => new Promise((done) => server.close(() => done())),
  };
}

const envelope = (dynamicWorkflow: unknown) => ({
  code: 0,
  data: { configs: { dynamicWorkflow } },
});

function tsService(stub: Stub, userMode: () => Promise<unknown>) {
  return createClientConfigService({
    apiClient: createNodeApiClient({ resolveESCodeEndpointOrigin: () => stub.origin }),
    resolveRequestContext: async () => ({
      endpointOrigin: stub.origin,
      appVersion: ESCODE_VERSION,
      platform: `${process.platform}-${process.arch}`,
    }),
    resolveDynamicWorkflowUserMode: userMode,
  });
}

const JSON_VALUE = (value: unknown) => JSON.parse(JSON.stringify(value));

const stub = await startStub();
const folds: Array<{ label: string; config: unknown }> = [];
const shortCircuit: Array<{ choice: string | null; config: unknown }> = [];
let userMode: unknown;
const ts = () => tsService(stub, async () => userMode);

const savedOverride = process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV];
try {
  const record = async (label: string) => {
    const expected = await ts().getDynamicWorkflowClientConfig({ forceRefresh: true });
    folds.push({ label, config: JSON_VALUE(expected) });
  };

  // 第一段（S3 主场景）：无本地覆盖。
  delete process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV];
  for (const remote of [
    { mode: "onDemand" },
    { mode: "alwaysOn" },
    { mode: "disabled" },
    { mode: " onDemand " },
    { mode: "bogus" },
    { mode: 1 },
    "x",
    null,
  ]) {
    stub.reply(200, envelope(remote));
    await record(`remote ${JSON.stringify(remote)}`);
  }
  stub.reply(200, { code: 0, data: { configs: {} } });
  await record("no dynamicWorkflow key");
  stub.reply(200, { code: 0, data: { configs: null } });
  await record("configs null");
  stub.reply(200, { code: 1, data: { configs: { dynamicWorkflow: { mode: "alwaysOn" } } } });
  await record("business code is not validated here");

  for (const offered of ["onDemand", "disabled"]) {
    stub.reply(200, envelope({ mode: offered }));
    for (const choice of ["alwaysOn", "onDemand", "disabled", "bogus", undefined]) {
      userMode = choice;
      await record(`offered ${offered}, user ${String(choice)}`);
    }
  }

  userMode = undefined;
  stub.reply(503, "{}");
  await record("http 503");
  stub.reply(200, "<html>");
  await record("unparseable body");

  // 第二段（S3 本地覆盖短路）：合法覆盖下不发请求。
  process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV] = "onDemand";
  stub.reply(200, envelope({ mode: "disabled" }));
  for (const choice of [undefined, "alwaysOn", "disabled"]) {
    userMode = choice;
    const expected = await ts().getDynamicWorkflowClientConfig({ forceRefresh: true });
    shortCircuit.push({ choice: choice ?? null, config: JSON_VALUE(expected) });
  }
} finally {
  if (savedOverride === undefined) delete process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV];
  else process.env[ESCODE_DYNAMIC_WORKFLOW_MODE_ENV] = savedOverride;
  await stub.close();
}

const golden = {
  _readme:
    "rustHostDynamicWorkflow.test.ts 的 TS getDynamicWorkflowClientConfig 观察冻结（Phase 3 golden freeze）。覆盖远端取值、用户选择、请求失败与本地覆盖短路；shortCircuit.choice 的 undefined 记为 null。",
  folds,
  shortCircuit,
};
const goldenDir = "packages/desktop/test/fixtures/golden";
const goldenPath = join(goldenDir, "rustHostDynamicWorkflow.oracle.json");
const json = `${JSON.stringify(golden, null, 2)}\n`;
if (process.argv.includes("--check")) {
  deepEqual(
    JSON.parse(await readFile(goldenPath, "utf8")),
    JSON.parse(json),
    "rustHostDynamicWorkflow golden 与当前 TS 观察漂移",
  );
  console.log("rustHostDynamicWorkflow golden: OK");
} else {
  await mkdir(goldenDir, { recursive: true });
  await writeFile(goldenPath, json);
  console.log("wrote", goldenPath);
}
