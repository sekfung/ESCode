import assert from "node:assert/strict";
import test from "node:test";
import { once } from "node:events";
import { createServer } from "node:https";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fixture, event, end } from "./zcode-cli-rust-fixture.js";
import { titleReply, titleRequest } from "./zcode-cli-rust-title-fixture.js";
import { configureRegistry } from "./zcode-cli-rust-registry-fixture.js";

const nodeBundle = resolve("apps/zcode-cli/packages/cli/dist/zcode.cjs");

/**
 * docs/specs/rust-net-proxy.md「Host 下发配置」：设置页的自定义 CA 由 services 注入
 * `NODE_EXTRA_CA_CERTS`（Node 启动即信任）与 `ZCODE_AGENT_CA_CERT`（跨运行时）。Rust 没有 Node 的
 * 原生行为，必须自己读后者——本用例把两个 runtime 都指向同一个自签名 TLS 模型服务，比较结果。
 */
test("settings-page CA is trusted by Node and Rust", async () => {
  const temp = await mkdtemp(join(tmpdir(), "rust-tls-ca-"));
  // 真实形态：设置页的 CA 签发的**叶子**证书（rustls 拒绝把 `CA:TRUE` 的证书当服务端叶子，
  // Node/OpenSSL 则宽容——单张自签证书会造出「Node 过、Rust 不过」的假差异）。
  const caKey = join(temp, "ca-key.pem"),
    caCert = join(temp, "ca.pem"),
    leafKey = join(temp, "leaf-key.pem"),
    leafCsr = join(temp, "leaf.csr"),
    leafCert = join(temp, "leaf.pem"),
    extensions = join(temp, "leaf.ext");
  const openssl = (...args: string[]) => promisify(execFile)("openssl", args);
  await openssl(
    "req", "-x509", "-newkey", "rsa:2048", "-nodes",
    "-keyout", caKey, "-out", caCert, "-days", "1",
    "-subj", "/CN=ZCode Test CA",
    "-addext", "basicConstraints=critical,CA:TRUE",
    "-addext", "keyUsage=critical,keyCertSign,cRLSign",
  );
  await openssl(
    "req", "-new", "-newkey", "rsa:2048", "-nodes",
    "-keyout", leafKey, "-out", leafCsr, "-subj", "/CN=127.0.0.1",
  );
  await writeFile(
    extensions,
    ["basicConstraints=critical,CA:FALSE", "keyUsage=critical,digitalSignature,keyEncipherment", "extendedKeyUsage=serverAuth", "subjectAltName=IP:127.0.0.1,DNS:localhost", ""].join("\n"),
  );
  await openssl(
    "x509", "-req", "-in", leafCsr, "-CA", caCert, "-CAkey", caKey,
    "-CAcreateserial", "-out", leafCert, "-days", "1", "-extfile", extensions,
  );
  const server = createServer({ key: await readFile(leafKey), cert: await readFile(leafCert) }, (req, res) => {
    req.setEncoding("utf8");
    let body = "";
    req.on("data", (chunk: string) => (body += chunk));
    req.on("end", () => {
      requests++;
      const request = JSON.parse(body);
      if (titleRequest(request)) return titleReply(res, request, "");
      res.writeHead(200, { "content-type": "text/event-stream" });
      event(res, { content: "trusted" });
      end(res, "stop");
    });
  });
  let tlsErrors = 0;
  let requests = 0;
  server.on("tlsClientError", () => {
    tlsErrors++;
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert(address && typeof address !== "string");

  const observed: string[] = [];
  try {
    for (const kind of ["node", "rust"] as const) {
      const f = await fixture(
        kind === "node"
          ? {
              command: process.execPath,
              args: ({ cwd }) => [nodeBundle, "app-server", "--stdio", "--cwd", cwd],
              registry: true,
              // 生产里 services 同时注入两者（Node 只认前者、Rust 只认后者）。
              env: { NODE_EXTRA_CA_CERTS: caCert, ZCODE_AGENT_CA_CERT: caCert },
            }
          : { registry: true, env: { ZCODE_AGENT_CA_CERT: caCert } },
      );
      try {
        const tlsBaseUrl = `https://127.0.0.1:${address.port}/v1`;
        // 两个 runtime 都用 App Provider Registry；把个人 provider 指向自签名 TLS 服务。
        await configureRegistry(f, false);
        const personalPath = join(f.root, "personal.json");
        const personal = JSON.parse(await readFile(personalPath, "utf8"));
        personal.config.providerConfigRules.providerRules[0].config.api.baseUrl = tlsBaseUrl;
        await writeFile(personalPath, JSON.stringify(personal));
        const h = f.start(),
          id = await h.create();
        await h.subscribe(`conversation/${id}`);
        await h.command(h.envelope("sendText", id, { text: "ca" }));
        const settled = await Promise.race([
          h.completed(id).then(() => "completed" as const),
          h
            .wait((m) =>
              m.params?.frame?.payload?.deltas?.some(
                (d: any) => d.patch?.control?.phase === "error",
              ),
            )
            .then(() => "error" as const),
        ]).catch((error: unknown) => `threw:${error instanceof Error ? error.message : error}`);
        const messages = JSON.stringify(h.messages);
        const detail = settled === "error" ? (messages.match(/"message":"([^"]{0,120})/) ?? [])[1] : "";
        observed.push(
          `${kind}:${
            settled === "completed"
              ? messages.includes("trusted")
                ? "trusted"
                : "missing"
              : `${settled}:${detail}`
          }`,
        );
        if (settled === "completed") assert.deepEqual(h.schemaErrors, []);
      } finally {
        await f.close();
      }
    }
    assert.deepEqual(
      observed,
      ["node:trusted", "rust:trusted"],
      `tlsErrors=${tlsErrors} requests=${requests}`,
    );
  } finally {
    server.closeAllConnections();
    await new Promise<void>((r) => server.close(() => r()));
    await rm(temp, { recursive: true, force: true });
  }
});

test("Rust retries socket failures, server errors and SSE network errors within one configured budget", async () => {
  const f = await fixture({
    env: { ZCODE_MODEL_RETRY_MAX_RETRIES: "0" },
    config: { retry: { maxRetries: 3, baseDelayMs: 1, maxDelayMs: 1, jitter: false } },
    respond(_req, res, attempt) {
      if (attempt === 1) {
        res.destroy();
        return;
      }
      if (attempt === 2) {
        res.writeHead(503);
        res.end('{"error":{"code":"500"}}');
        return;
      }
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      if (attempt === 3) {
        res.end('data: {"error":{"code":"ECONNRESET","message":"private diagnostic"}}\n\n');
        return;
      }
      event(res, { content: "recovered" });
      end(res, "stop");
    },
  });
  try {
    const h = f.start(),
      id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "retry" }));
    await h.completed(id);
    assert.equal(f.requests.length, 4);
    assert.equal(new Set(f.requestBodies).size, 1);
    const retries = h.messages
      .flatMap((m) => m.params?.frame?.payload?.deltas ?? [])
      .map((d: any) => d.patch?.control?.apiRetry)
      .filter(Boolean);
    assert.deepEqual(
      retries.map((r: any) => r.reasonCode),
      // V4 reasonCode 与 Node modelRetryReasonCode 一致（rust-model-retry.md）。
      ["fault.network.unreachable", "fault.provider.serverError", "fault.network.unreachable"],
    );
    assert(!JSON.stringify(h.messages).includes("private diagnostic"));
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
  }
});

test("Rust TLS certificate failure is terminal and redacted", async () => {
  const temp = await mkdtemp(join(tmpdir(), "rust-tls-"));
  const key = join(temp, "key.pem"),
    cert = join(temp, "cert.pem");
  // 测试期生成自签名证书；不在仓库保存私钥，也不请求外部供应商。
  await promisify(execFile)("openssl", [
    "req",
    "-x509",
    "-newkey",
    "rsa:2048",
    "-nodes",
    "-keyout",
    key,
    "-out",
    cert,
    "-days",
    "1",
    "-subj",
    "/CN=localhost",
  ]);
  let handshakes = 0;
  const server = createServer({ key: await readFile(key), cert: await readFile(cert) });
  server.on("tlsClientError", () => {
    handshakes++;
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert(address && typeof address !== "string");
  const f = await fixture();
  try {
    const config = JSON.parse(await readFile(f.config, "utf8"));
    await writeFile(
      f.config,
      JSON.stringify({
        ...config,
        baseUrl: `https://127.0.0.1:${address.port}/v1`,
        retry: { maxRetries: 2, baseDelayMs: 1, jitter: false },
      }),
    );
    const h = f.start(),
      id = await h.create();
    await h.subscribe(`conversation/${id}`);
    await h.command(h.envelope("sendText", id, { text: "tls" }));
    const failure = await h.wait((m) =>
      m.params?.frame?.payload?.deltas?.some((d: any) => d.patch?.control?.phase === "error"),
    );
    const error = failure.params.frame.payload.deltas.find((d: any) => d.patch?.control?.lastError)
      .patch.control.lastError;
    assert.equal(error.attribution.reason, "tls_error");
    assert.equal(error.attribution.retryable, false);
    assert.equal(handshakes, 1);
    assert(!JSON.stringify(h.messages).includes(String(address.port)));
    assert.deepEqual(h.schemaErrors, []);
  } finally {
    await f.close();
    server.closeAllConnections();
    await new Promise<void>((r) => server.close(() => r()));
    await rm(temp, { recursive: true, force: true });
  }
});
