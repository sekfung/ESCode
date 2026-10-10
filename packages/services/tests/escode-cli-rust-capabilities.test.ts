import assert from "node:assert/strict";
import test from "node:test";
import {
  readRuntimeCapabilities,
  ensureIndependentPlanSupport,
} from "../src/escode-agent/independentPlanSupport.js";
import type { ESCodeProtocolClient } from "../src/escode-agent/escodeProtocolClient.js";

test("Runtime capability negotiation preserves legacy overlay behavior and caches per connection", async () => {
  let calls = 0;
  const legacy: Pick<ESCodeProtocolClient, "request"> = {
    async request() {
      calls++;
      throw Object.assign(new Error("old runtime"), { code: -32601 });
    },
  };
  assert.deepEqual(await readRuntimeCapabilities(legacy), {});
  assert.equal((await readRuntimeCapabilities(legacy)).accountProviderConfig, undefined);
  await assert.rejects(ensureIndependentPlanSupport(legacy), /independentPlanUnsupported/);
  assert.equal(calls, 1);
  const failed: Pick<ESCodeProtocolClient, "request"> = {
    async request() {
      calls++;
      throw Object.assign(new Error("transport failed"), { code: -32000 });
    },
  };
  await assert.rejects(readRuntimeCapabilities(failed), /transport failed/);
  await assert.rejects(readRuntimeCapabilities(failed), /transport failed/);
  assert.equal(calls, 3, "Transport failures must be retried rather than cached as capabilities");
});
