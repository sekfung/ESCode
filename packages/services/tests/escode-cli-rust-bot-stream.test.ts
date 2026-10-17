import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import { fixture, binary } from "./escode-cli-rust-fixture.js";
import type { ModelSelectionView } from "@escode/provider";
import { completeNewModelSelection } from "@escode/provider";
import type { ESCodeStreamEvent } from "@escode/shared";
import type { CommandEnvelope } from "@escode/shared/escode-protocol-v4";
import { createComposerSubmissionConfig } from "../../ui/src/v4/composer/composerSubmissionConfig.js";

// docs/specs/bots-v4-stream.md §6：真实 Rust runtime 的 V4 帧经持续订阅 + Bot 投影，
// 得到 Bot 回复所需的正文分段与完成事件（旧 legacy session/subscribe 在 Rust 上不存在）。
test("Bot stream projection receives text and completion from the real Rust runtime", async () => {
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
  const { observeConversationFrames } =
    await import("../src/escode-agent/conversationFrameFeed.js");
  const { createBotConversationStream } = await import("../src/bots/botConversationStream.js");
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
        throw new Error("Native core must not request an account overlay");
      },
      onDidChange: () => () => {},
    },
  });
  try {
    const workspace = { workspacePath: f.cwd };
    assert.equal((await service.initialize(workspace)).available, true);
    const envelope = (
      type: CommandEnvelope["type"],
      sessionId: string | null,
      payload = {},
      commandId: string = randomUUID(),
    ): CommandEnvelope => ({
      commandId,
      clientId: "bot-fixture",
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
    assert(submission);
    const created = await service.sendConversationCommandV4({
      ...workspace,
      envelope: envelope("createSession", null, { workspaceId: f.cwd, config: submission }),
    });
    if (created.result?.type !== "createSession") throw new Error("Session not created");
    const sessionId = created.result.sessionId;
    const projection = createBotConversationStream(sessionId);
    const events: ESCodeStreamEvent[] = [];
    let snapshotSeen!: () => void;
    const ready = new Promise<void>((resolve) => (snapshotSeen = resolve));
    let finished!: () => void;
    const done = new Promise<void>((resolve) => (finished = resolve));
    const subscription = observeConversationFrames(service, { ...workspace, sessionId })((
      frame,
    ) => {
      if (frame.payload.kind === "snapshot") snapshotSeen();
      for (const event of projection.accept(frame)) {
        events.push(event);
        if (event.type === "task_complete" || event.type === "task_error") finished();
      }
    });
    let timer: NodeJS.Timeout | undefined;
    try {
      await ready;
      await service.sendConversationCommandV4({
        ...workspace,
        envelope: envelope("sendText", sessionId, { text: "hello", ...submission }, "bot-input-1"),
      });
      await Promise.race([
        done,
        new Promise((_resolve, reject) => {
          timer = setTimeout(() => reject(new Error(JSON.stringify(events))), 8000);
        }),
      ]);
    } finally {
      clearTimeout(timer);
      subscription.dispose();
    }
    const started = events.find((event) => event.type === "task_run_started");
    assert.equal(started?.inputId, "bot-input-1");
    const text = events
      .flatMap((event) => (event.type === "agent_message_chunk" ? [event.content] : []))
      .join("");
    assert.equal(text, "你好 Rust");
    assert.equal(events.at(-1)?.type, "task_complete");
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
