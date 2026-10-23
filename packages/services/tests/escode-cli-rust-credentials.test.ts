import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";
import { once } from "node:events";
import { canonical, nodeOracle } from "./escode-cli-rust-golden.js";

// docs/specs/rust-mcp-oauth.md「兼容点」1、3、4：Rust 与 Node 共用同一份加密凭据文件。
// Phase 3 之后正常跑只执行 Rust：TS 侧的密文、路径推导与种子凭据文件已冻结为 golden。
// 使用固定 secret（`ESCODE_CREDENTIAL_SECRET`），否则默认 secret 含平台/家目录/用户名，
// 冻结的密文无法跨机器回放。
const SECRET = "cli-golden-fixture-secret";
const cipherModule = "../../../apps/escode-cli/packages/adapters/src/auth/credential-cipher.js";
const storeModule = "../../../apps/escode-cli/packages/adapters/src/auth/shared-credentials.js";
const portable = (path: string) => path.split(sep).join("/");

function driver(file: string, env: Record<string, string | undefined> = process.env) {
  const child = spawn(
    resolve(
      `apps/escode-cli-rust/target/debug/examples/credential_fixture${process.platform === "win32" ? ".exe" : ""}`,
    ),
    [file],
    { env: env as NodeJS.ProcessEnv },
  );
  let errors = "";
  child.stderr.on("data", (s) => (errors += s));
  const lines = createInterface({ input: child.stdout });
  const pending: ((value: any) => void)[] = [];
  lines.on("line", (line) => pending.shift()?.(JSON.parse(line)));
  const call = (op: Record<string, unknown>) =>
    new Promise<any>((resolveCall) => {
      pending.push(resolveCall);
      child.stdin.write(`${JSON.stringify(op)}\n`);
    }).then((reply) => {
      if (reply.error) throw new Error(`${reply.error}\n${errors}`);
      return reply.ok;
    });
  return {
    call,
    async close() {
      const closed = once(child, "close");
      child.stdin.end();
      assert.deepEqual(await closed, [0, null], errors);
    },
  };
}

test("Rust decrypts frozen Node ciphertext with the shared AES-256-GCM format", async () => {
  const root = await mkdtemp(join(tmpdir(), "escode-cred-cipher-"));
  const env = { ...process.env, ESCODE_CREDENTIAL_SECRET: SECRET };
  const rust = driver(join(root, "credentials.json"), env);
  try {
    const value = "tok-值-🙂";
    // Node 加密使用随机 IV，只有密文本身可冻结；Rust 加密方向改为同格式 + Rust 自解密。
    const nodeCiphertext = await nodeOracle("ciphertext", async () => {
      const { createESCodeCredentialCipher } = await import(cipherModule);
      return createESCodeCredentialCipher({ env }).encrypt(value);
    });
    assert.equal(await rust.call({ op: "decrypt", value: nodeCiphertext }), value);
    const rustCiphertext = await rust.call({ op: "encrypt", value });
    assert.match(rustCiphertext, /^enc:v1:[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/);
    assert.equal(await rust.call({ op: "decrypt", value: rustCiphertext }), value);
  } finally {
    await rust.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("Rust reads the frozen Node credential file, keeps its path and shares a cross-process lock", async () => {
  const root = await mkdtemp(join(tmpdir(), "escode-cred-store-"));
  const env: Record<string, string | undefined> = {
    ...process.env,
    ESCODE_DATA_BASE_DIR: root,
    ESCODE_CREDENTIAL_SECRET: SECRET,
  };
  // TS 路径推导：用固定基准目录录制，正常跑只拼接到当前临时根。
  const pathShape = await nodeOracle("path", async () => {
    const { resolveSharedESCodeCredentialsPath } = await import(storeModule);
    const base = "/golden-data-base";
    const file = resolveSharedESCodeCredentialsPath({
      env: { ...env, ESCODE_DATA_BASE_DIR: base },
    });
    return portable(relative(base, file));
  });
  const file = join(root, ...pathShape.split("/"));
  assert.equal(file, join(root, ".escode", "v2", "credentials.json"));
  // 冻结 Node store 写出的种子凭据文件；正常跑把它复制到临时目录，不再执行 TS。
  const seed = await nodeOracle("seed", async () => {
    const { createSharedESCodeCredentialStore } = await import(storeModule);
    const node = createSharedESCodeCredentialStore({ env });
    await node.save("mcp:oauth:abc:tokens", '{"access_token":"n"}');
    return JSON.parse(await readFile(file, "utf8")) as Record<string, string>;
  });
  if (process.env.ESCODE_RECORD_CLI_GOLDEN !== "1") {
    await mkdir(dirname(file), { recursive: true });
    await writeFile(file, `${JSON.stringify(seed, null, 2)}\n`, { mode: 0o600 });
  }
  const rust = driver(file, env);
  let peer: ReturnType<typeof driver> | undefined;
  try {
    assert.equal(
      await rust.call({ op: "load", key: "mcp:oauth:abc:tokens" }),
      '{"access_token":"n"}',
    );
    await rust.call({
      op: "save",
      entries: [["mcp:oauth:abc:client_information", '{"client_id":"r"}']],
    });
    assert.equal(
      canonical(await rust.call({ op: "load", key: "mcp:oauth:abc:client_information" })),
      '{"client_id":"r"}',
    );
    // 交错的独立 read-modify-write：锁失效时后写者会用旧快照覆盖对方的 key。
    // Node 侧写入者已冻结；这里用第二个 Rust 进程保留跨进程锁覆盖。
    peer = driver(file, env);
    const count = 25;
    await Promise.all([
      rust.call({ op: "burst", prefix: "rust-", count }),
      peer.call({ op: "burst", prefix: "node-", count }),
    ]);
    const all = JSON.parse(await readFile(file, "utf8")) as Record<string, string>;
    for (let i = 0; i < count; i += 1) {
      assert.ok(all[`rust-${i}`]?.startsWith("enc:v1:"), `rust-${i}`);
      assert.ok(all[`node-${i}`]?.startsWith("enc:v1:"), `node-${i}`);
    }
    assert.equal(await rust.call({ op: "load", key: "rust-7" }), "v7");
    assert.equal(await rust.call({ op: "load", key: "node-7" }), "v7");
    // 锁目录在释放后不残留。
    assert.ok(!(await readdir(join(root, ".escode", "v2"))).some((n) => n.endsWith(".lock")));
  } finally {
    await peer?.close();
    await rust.close();
    await rm(root, { recursive: true, force: true });
  }
});
