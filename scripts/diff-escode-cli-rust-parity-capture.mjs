// 差分垂直切片的捕获层：在确定性环境里驱动一侧 runtime（Node bundle 或 Rust 二进制），
// 记录未归一的原始事实（V4 帧原文、mock provider 收到的请求体、退出码、存储/工作区文件清单）。
// 两侧用同一 mock SSE、同一临时 HOME/workspace/storage、同一驱动脚本与固定输入。

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdir, mkdtemp, readdir, readFile, realpath, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";

const finish =
  'data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":4}}\n\ndata: [DONE]\n\n';

const deterministicUuid = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;

/** 目录清单：相对路径（/ 分隔）+ 字节数 + sha256。 */
export async function inventory(dir) {
  const files = [];
  const walk = async (current) => {
    for (const entry of await readdir(current, { withFileTypes: true }).catch(() => [])) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) await walk(path);
      else {
        const info = await stat(path);
        files.push({
          path: relative(dir, path).replaceAll("\\", "/"),
          bytes: info.size,
          sha256: createHash("sha256")
            .update(await readFile(path))
            .digest("hex"),
        });
      }
    }
  };
  await walk(dir);
  return files.sort((a, b) => a.path.localeCompare(b.path));
}

/** sqlite 文件级事实（表清单/行级的解析对拍待存储跨读轮接入）。 */
export async function sqliteFacts(dir) {
  const facts = [];
  for (const file of await inventory(dir)) {
    if (!/\.(sqlite|db|sqlite3)$/.test(file.path)) continue;
    const bytes = await readFile(join(dir, file.path));
    facts.push({
      path: file.path,
      bytes: file.bytes,
      header: bytes.subarray(0, 16).toString("latin1"),
    });
  }
  return facts;
}

async function writeFixture(repoRoot, capRoot, baseUrl) {
  const builtin = JSON.parse(
    await readFile(resolve(repoRoot, "config/provider/escode-builtin.json"), "utf8"),
  );
  builtin.config.providerConfigRules.providerRules = [];
  const builtinFile = join(capRoot, "builtin.json");
  const personalFile = join(capRoot, "personal.json");
  const rustConfig = join(capRoot, "rust-model.json");
  await Promise.all([
    writeFile(builtinFile, JSON.stringify(builtin)),
    writeFile(
      personalFile,
      JSON.stringify({
        schemaVersion: 1,
        config: {
          providerConfigRules: {
            providerRules: [
              {
                providerId: "personal:fixture",
                providerName: "Fixture",
                config: {
                  group: "standard-personal",
                  access: { type: "api-key", apiKey: "fixture-only" },
                  api: { type: "openai-chat-completions", baseUrl },
                  personalModelIds: ["model-a"],
                },
              },
            ],
          },
          modelConfigRules: {
            providerModelRules: [
              {
                providerId: "personal:fixture",
                modelId: "model-a",
                config: {
                  properties: { contextWindow: 256000 },
                  optionSpecs: { reasoningLevel: { values: ["none"], map: "{}" } },
                },
              },
            ],
            manualProviderModelRules: [],
          },
          defaultModelSelection: {
            providerId: "personal:fixture",
            modelId: "model-a",
            options: { reasoningLevel: "none" },
          },
        },
      }),
    ),
    writeFile(
      rustConfig,
      JSON.stringify({
        apiType: "openai-chat-completions",
        providerId: "personal:fixture",
        modelId: "model-a",
        reasoningLevel: "none",
        contextWindow: 256000,
        baseUrl,
      }),
    ),
  ]);
  return { builtinFile, personalFile, rustConfig };
}

/** mock provider：记录全部请求体；标题请求两侧形态不同（Node 非流式 / Rust 流式）按请求应答。 */
async function startMockProvider(modelRequests) {
  const server = createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    modelRequests.push({
      method: request.method,
      url: request.url,
      contentType: request.headers["content-type"] ?? "",
      at: Date.now(),
      title: body.includes("Generate a concise title for this coding session"),
      body,
    });
    const parsed = JSON.parse(body);
    if (body.includes("Generate a concise title for this coding session")) {
      if (parsed.stream === true) {
        response.writeHead(200, { "Content-Type": "text/event-stream" });
        response.end(
          `data: ${JSON.stringify({ choices: [{ delta: { content: "Parity" } }] })}\n\n${finish}`,
        );
      } else {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(
          JSON.stringify({
            choices: [{ message: { role: "assistant", content: "Parity" }, finish_reason: "stop" }],
          }),
        );
      }
      return;
    }
    response.writeHead(200, { "Content-Type": "text/event-stream" });
    for (let index = 0; index < 4; index++) {
      const sse = `data: ${JSON.stringify({ choices: [{ delta: { content: `chunk-${index} ` } }] })}\n\n`;
      if (!response.write(sse)) await once(response, "drain");
    }
    response.end(finish);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  return { server, baseUrl: `http://127.0.0.1:${server.address().port}/v1` };
}

/** 跑一侧：capabilities → createSession → subscribe → sendText → 等 completedSuccess → 关闭。 */
export async function runSide(options) {
  const { side, repoRoot, nodeBundle, rustBinary, keep } = options;
  // 修复：Windows runner 的 tmpdir() 是 8.3 短路径；Node 子进程对其 fs.watch 触发 libuv 断言
  // `!_wcsnicmp(filename, dir, dirlen)` 崩溃，本脚本随即空等到作业超时（bench windows 被取消）。
  // promises.realpath 走原生实现，展开为长路径（与 bench-escode-cli-node-rust.mjs 同一修复）。
  const capRoot = await realpath(await mkdtemp(join(tmpdir(), `escode-parity-${side}-`)));
  const home = join(capRoot, "home");
  const workspace = join(capRoot, "workspace");
  const storage = join(capRoot, "storage");
  await Promise.all([mkdir(home), mkdir(workspace), mkdir(storage)]);

  const modelRequests = [];
  const { server, baseUrl } = await startMockProvider(modelRequests);
  const { builtinFile, personalFile, rustConfig } = await writeFixture(repoRoot, capRoot, baseUrl);

  const command = side === "node" ? process.execPath : rustBinary;
  const args =
    side === "node"
      ? [nodeBundle, "app-server", "--stdio", "--surface", "terminal", "--cwd", workspace]
      : [
          "app-server",
          "--stdio",
          "--surface",
          "terminal",
          "--cwd",
          workspace,
          "--data-dir",
          join(capRoot, "rust-data"),
          "--config",
          rustConfig,
        ];
  const child = spawn(command, args, {
    cwd: repoRoot,
    stdio: ["pipe", "pipe", "pipe"],
    env: {
      ...process.env,
      NO_PROXY: "127.0.0.1,localhost",
      no_proxy: "127.0.0.1,localhost",
      HOME: home,
      USERPROFILE: home,
      XDG_CONFIG_HOME: join(home, ".config"),
      ESCODE_STORAGE_DIR: storage,
      ESCODE_SESSION_DB_PATH: join(storage, "node.sqlite"),
      ESCODE_BUILTIN_PROVIDER_CONFIG_FILE: builtinFile,
      ESCODE_PERSONAL_PROVIDER_CONFIG_FILE: personalFile,
      ESCODE_WORKSPACE_IDENTITY: workspace,
    },
  });

  const frames = [];
  const frameTimes = [];
  const inboundRpcs = [];
  let stderr = "";
  let buffer = "";
  let serial = 0;
  let turnDone = null;
  const pending = new Map();
  child.stderr.setEncoding("utf8").on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-4000);
  });
  child.stdout.setEncoding("utf8").on("data", (chunk) => {
    buffer += chunk;
    let newline;
    while ((newline = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      frames.push(line);
      frameTimes.push(Date.now());
      let frame;
      try {
        frame = JSON.parse(line);
      } catch {
        continue;
      }
      if (frame.id !== undefined && frame.method) {
        // host→runtime 请求：全部记录；偏好请求回固定答案，未知请求回空结果（都进入对拍可见面）。
        inboundRpcs.push(frame.method);
        const result =
          frame.method === "session/requestRuntimePreferences"
            ? {
                askUserQuestionAutoResolutionEnabled: true,
                nativeSearchEnhancementsEnabled: true,
                memoryEnabled: false,
              }
            : {};
        child.stdin.write(`${JSON.stringify({ id: frame.id, result })}\n`);
        continue;
      }
      if (frame.id !== undefined) {
        const waiter = pending.get(frame.id);
        pending.delete(frame.id);
        if (frame.error) waiter?.reject(new Error(frame.error.message));
        else waiter?.resolve(frame.result);
        continue;
      }
      for (const delta of frame.params?.frame?.payload?.deltas ?? []) {
        if (delta.patch?.control?.phase === "completedSuccess") turnDone?.resolve();
        if (delta.patch?.control?.phase === "error")
          turnDone?.reject(new Error(JSON.stringify(delta.patch.control.lastError)));
      }
    }
  });

  const rpc = (method, params = {}) => {
    const id = ++serial;
    return new Promise((settle, fail) => {
      pending.set(id, { resolve: settle, reject: fail });
      child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
    });
  };
  const commandRpc = (type, sessionId, payload, ordinal) =>
    rpc("v4/command", {
      commandId: deterministicUuid(ordinal),
      clientId: "parity",
      sessionId,
      type,
      payload,
      issuedAt: 0,
    });

  const started = Date.now();
  const result = {
    side,
    root: capRoot,
    startedAt: started,
    frames,
    frameTimes,
    inboundRpcs,
    modelRequests,
    exitCode: null,
    signal: null,
    startupMs: null,
    wallMs: null,
    stderrTail: "",
    workspaceFiles: [],
    storageFacts: { inventory: [], sqlite: [] },
  };
  try {
    await rpc("runtime/capabilities");
    result.startupMs = Date.now() - started;
    const created = await commandRpc("createSession", null, { workspaceId: workspace }, 1);
    if (!created?.result?.sessionId) {
      throw new Error(`createSession 响应异常：${JSON.stringify(created)}`);
    }
    await rpc("v4/conversation/subscribe", {
      topic: `conversation/${created.result.sessionId}`,
      connectionId: "parity",
      clientMode: "desktop-continuous",
    });
    const done = new Promise((settle, fail) => (turnDone = { resolve: settle, reject: fail }));
    await commandRpc("sendText", created.result.sessionId, { text: "parity turn" }, 2);
    await Promise.race([
      done,
      new Promise((_, fail) => setTimeout(() => fail(new Error("turn 未在 60s 内完成")), 60_000)),
    ]);
    // 尾帧静默期：completedSuccess 之后仍有收尾通知（终态遥测/末帧），立即关 stdin 会与它们竞态，
    // 帧数抖动会让差分指纹不稳定。等最后一条帧 400ms（上限 3s）再关闭。
    const quiescenceDeadline = Date.now() + 3000;
    let lastFrameAt = frameTimes.at(-1) ?? 0;
    while (Date.now() - lastFrameAt < 400 && Date.now() < quiescenceDeadline) {
      await new Promise((settle) => setTimeout(settle, 50));
      const latest = frameTimes.at(-1) ?? 0;
      if (latest !== lastFrameAt) lastFrameAt = latest;
    }
  } finally {
    child.stdin.end();
    const closed = once(child, "close");
    const timedOut = await Promise.race([
      closed.then(() => false),
      new Promise((settle) => setTimeout(() => settle(true), 15_000)),
    ]);
    if (timedOut) child.kill("SIGKILL");
    const [code, signal] = await closed;
    result.exitCode = code ?? null;
    result.signal = signal ?? null;
    result.wallMs = Date.now() - started;
    result.stderrTail = stderr.split("\n").slice(-6).join("\n");
    result.workspaceFiles = await inventory(workspace);
    // Rust 侧的存储落在 --data-dir（rust-data），Node 侧在 ESCODE_STORAGE_DIR；各取各自实际目录。
    result.storageRoot = side === "node" ? storage : join(capRoot, "rust-data");
    result.storageFacts = {
      inventory: await inventory(result.storageRoot),
      sqlite: await sqliteFacts(result.storageRoot),
    };
    server.closeAllConnections();
    await new Promise((settle) => server.close(settle));
    if (!keep) await rm(capRoot, { recursive: true, force: true });
  }
  return result;
}
