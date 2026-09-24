import assert from "node:assert/strict";
import test from "node:test";
import {
  zcodeWorkspacePresentationSchema,
  supportsRuntimeExecution,
  runtimeExecutionCapabilitiesSchema,
} from "@zcode/shared";
import { fixture } from "./zcode-cli-rust-fixture.js";

// build/edit 的判定、确认交互与项目规则持久化已实现；独立 plan 状态与审批已与 Node 差分一致
// （rust-plan-mode.md）；auto 在 TS 同样保留未实现。
const native = runtimeExecutionCapabilitiesSchema.parse({
  permissionModes: ["yolo", "build", "edit"],
  independentPlanState: true,
});

test("Execution capabilities gate permissions and Plan without changing the user's intent", () => {
  const build = { mode: "build", planEnabled: false };
  assert.equal(supportsRuntimeExecution(build, native), true);
  assert.deepEqual(build, { mode: "build", planEnabled: false });
  assert.equal(supportsRuntimeExecution({ mode: "edit", planEnabled: false }, native), true);
  assert.equal(supportsRuntimeExecution({ mode: "yolo", planEnabled: false }, native), true);
  assert.equal(supportsRuntimeExecution({ mode: "yolo", planEnabled: true }, native), true);
  // 旧的 mode=plan 解析为 build + planEnabled，两者均已宣告。
  assert.equal(supportsRuntimeExecution({ mode: "plan" }, native), true);
  assert.equal(
    supportsRuntimeExecution(
      { mode: "yolo", planEnabled: true },
      { ...native, independentPlanState: false },
    ),
    false,
  );
  assert.equal(supportsRuntimeExecution(build), true);
  assert.equal(supportsRuntimeExecution({ mode: "plan" }), true);
  assert.equal(
    runtimeExecutionCapabilitiesSchema.safeParse({ ...native, permissionModes: [] }).success,
    false,
  );
  assert.equal(
    runtimeExecutionCapabilitiesSchema.safeParse({ ...native, permissionModes: ["plan"] }).success,
    false,
  );
});

test("Native workspace presentation and both delivery subscriptions expose the same capability set", async () => {
  const f = await fixture();
  try {
    const h = f.start();
    const presentation = await h.client.request(
      "workspace/readPresentation",
      {
        workspace: { workspacePath: f.cwd, workspaceKey: f.cwd },
        includeExecutionCapabilities: true,
      },
      zcodeWorkspacePresentationSchema,
    );
    assert.deepEqual(presentation.executionCapabilities, native);
    const legacy = await h.client.request(
      "workspace/readPresentation",
      {
        workspace: { workspacePath: f.cwd, workspaceKey: f.cwd },
      },
      zcodeWorkspacePresentationSchema.omit({ executionCapabilities: true }).strict(),
    );
    assert.equal("executionCapabilities" in legacy, false);
    for (const delivery of ["desktop-continuous", "web-remote-replayable"]) {
      const ack = await h.subscribe(`workspace-config/${f.cwd}`, `modes-${delivery}`, delivery);
      const frame = await h.wait(
        (m) =>
          m.params?.subscriptionId === ack.ack.subscriptionId &&
          m.params?.topic === `workspace-config/${f.cwd}`,
      );
      assert.deepEqual(frame.params.frame.payload.snapshot.config.executionCapabilities, native);
    }
    assert.equal(f.requests.length, 0);
    // build/edit 已支持：建会话不再被拒；plan 与 auto 仍按未实现显式拒绝。
    const created = await h.command(
      h.envelope("createSession", null, {
        workspaceId: f.cwd,
        config: { mode: "build" },
      }),
    );
    assert.equal(created.status, "accepted");
    await assert.rejects(
      h.command(
        h.envelope("createSession", null, {
          workspaceId: f.cwd,
          config: { mode: "plan" },
          firstInput: { text: "never execute" },
        }),
      ),
      /Unsupported core execution mode/,
    );
    assert.equal(f.requests.length, 0);
  } finally {
    await f.close();
  }
});
