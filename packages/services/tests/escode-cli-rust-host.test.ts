import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import { fixture, binary } from "./escode-cli-rust-fixture.js";
import type { ModelSelectionView } from "@escode/provider";
import { completeNewModelSelection } from "@escode/provider";
import { createComposerSubmissionConfig } from "../../ui/src/v4/composer/composerSubmissionConfig.js";
import {
  conversationTopicFrameSchema,
  type CommandEnvelope,
} from "@escode/shared/escode-protocol-v4";

test("Host service uses the real native runtime", async () => {
  const f = await fixture();
  const keys = [
    "ESCODE_AGENT_SERVER_COMMAND",
    "ESCODE_AGENT_SERVER_ARGS_JSON",
    "ESCODE_AGENT_SERVER_RUNTIME",
    "ESCODE_DATA_BASE_DIR",
    "HOME",
  ] as const;
  const previous = Object.fromEntries(keys.map((key) => [key, process.env[key]]));
  Object.assign(process.env, {
    ESCODE_AGENT_SERVER_COMMAND: binary,
    ESCODE_AGENT_SERVER_ARGS_JSON: JSON.stringify([
      "app-server",
      "--stdio",
      "--data-dir",
      f.dataDir,
      "--config",
      f.config,
    ]),
    ESCODE_AGENT_SERVER_RUNTIME: "escode-cli-rust",
    ESCODE_DATA_BASE_DIR: f.root,
    HOME: f.root,
  });
  const { setDataBaseDir } = await import("../src/paths.js");
  setDataBaseDir(f.root);
  const { createESCodeAgentService } = await import("../src/escode-agent/escodeAgentService.js");
  let accountReads = 0;
  const view: ModelSelectionView = {
    revision: 1,
    providers: [
      {
        providerId: "fixture",
        config: {},
        models: [
          {
            modelId: "core-model",
            config: {
              enabled: true,
              properties: {
                requiresMfjsToolSchema: false,
                contextWindow: 128000,
                inputFormat: {
                  supportsText: true,
                  supportsImage: false,
                  supportsVideo: false,
                  supportsAudio: false,
                  supportsPdf: false,
                },
                outputFormat: { supportsText: true },
                supportsToolCall: true,
                supportsJsonSchemaOutput: false,
                supportsNativeWebSearch: false,
                supportsMidConversationSystem: false,
              },
              optionSpecs: {
                reasoningLevel: { values: ["none"], map: "{}" },
                maxOutputTokens: { max: 8192, map: "{}" },
              },
            },
          },
        ],
      },
    ],
  };
  const service = createESCodeAgentService({
    presentationSurface: "desktop",
    requestTimeoutMs: 5000,
    modelSelectionReadinessSource: { getView: async () => view },
    accountProviderConfigSource: {
      async read() {
        accountReads++;
        throw new Error("Native core must not request an account overlay");
      },
      onDidChange: () => () => {},
    },
  });
  try {
    const workspace = { workspacePath: f.cwd };
    assert.equal((await service.initialize(workspace)).available, true);
    assert.deepEqual((await service.readWorkspacePresentation(workspace)).executionCapabilities, {
      // build/edit/guarded 与独立 plan 状态均已实现（见 rust-plan-mode.md、rust-v3.15.1-drift.md）。
      permissionModes: ["yolo", "build", "edit", "guarded"],
      independentPlanState: true,
    });
    const envelope = (
      type: CommandEnvelope["type"],
      sessionId: string | null,
      payload = {},
    ): CommandEnvelope => ({
      commandId: randomUUID(),
      clientId: "host-fixture",
      sessionId,
      type,
      payload,
      issuedAt: Date.now(),
    });
    const selection = completeNewModelSelection(view, {
      providerId: "fixture",
      modelId: "core-model",
    });
    const submission = createComposerSubmissionConfig(
      { mode: "yolo", planEnabled: false, modelSelection: selection },
      view,
    );
    assert(submission, "Real App composer must permit the selected model");
    const capabilities = {
      permissionModes: ["yolo" as const, "build" as const, "edit" as const],
      independentPlanState: true,
    };
    // build/edit 与独立 plan 状态均已宣告，真实 App Composer 允许提交 planEnabled。
    assert.equal(
      createComposerSubmissionConfig({ ...submission, mode: "build" }, view, capabilities)?.mode,
      "build",
    );
    assert.equal(
      createComposerSubmissionConfig({ ...submission, mode: "edit" }, view, capabilities)?.mode,
      "edit",
    );
    assert.equal(
      createComposerSubmissionConfig({ ...submission, planEnabled: true }, view, capabilities)
        ?.planEnabled,
      true,
    );
    assert.equal(createComposerSubmissionConfig(submission, view, capabilities)?.mode, "yolo");
    const created = await service.sendConversationCommandV4({
      ...workspace,
      envelope: envelope("createSession", null, { workspaceId: f.cwd, config: submission }),
    });
    assert.equal(created.result?.type, "createSession");
    if (created.result?.type !== "createSession") throw new Error("Session not created");
    const sessionId = created.result.sessionId;
    let timer: NodeJS.Timeout;
    let subscription: { dispose(): void };
    const completed = new Promise<void>((resolve, reject) => {
      timer = setTimeout(
        () => reject(new Error("Host did not receive the completed projection")),
        8000,
      );
      subscription = service.onDynamicConversationFrame(workspace)((wire) => {
        if (wire.kind !== "complete") return;
        const frame = conversationTopicFrameSchema.parse(wire.frame);
        if (
          frame.payload.kind === "deltas" &&
          frame.payload.deltas.some(
            (delta) =>
              delta.op === "state.updated" && delta.patch.control?.phase === "completedSuccess",
          )
        )
          resolve();
      });
    });
    try {
      await service.subscribeConversationV4({ ...workspace, sessionId });
      assert.equal(
        (
          await service.sendConversationCommandV4({
            ...workspace,
            envelope: envelope("sendText", sessionId, { text: "hello", ...submission }),
          })
        ).status,
        "accepted",
      );
      await completed;
    } finally {
      clearTimeout(timer!);
      subscription!.dispose();
    }
    assert.equal(accountReads, 0);
    assert.equal(f.requests.length, 1);
    assert.equal(f.requests[0]!.reasoning_effort, "none");
    await assert.rejects(
      service.sendConversationCommandV4({
        ...workspace,
        envelope: envelope("sendText", sessionId, {
          text: "unsupported",
          ...submission,
          modelSelection: { ...submission.modelSelection, options: { reasoningLevel: "high" } },
        }),
      }),
      /unsupported/i,
    );
  } finally {
    await service.disposeAllAndWait();
    for (const key of keys) {
      const value = previous[key];
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    setDataBaseDir(null);
    await f.close();
  }
});
