import assert from "node:assert/strict";
import test from "node:test";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { once } from "node:events";
import {
  createSharedZCodeCredentialStore,
  resolveSharedZCodeCredentialsPath,
} from "../../../apps/zcode-cli/packages/adapters/src/auth/shared-credentials.js";
import { createZCodeCredentialCipher } from "../../../apps/zcode-cli/packages/adapters/src/auth/credential-cipher.js";

// docs/specs/rust-mcp-oauth.md「兼容点」1、3、4：Rust 与 Node 共用同一份加密凭据文件。
// 使用默认 secret（不设 ZCODE_CREDENTIAL_SECRET），验证 platform/homedir/username 推导与 Node 一致。
function driver(file: string, env: Record<string, string | undefined> = process.env) {
  const child = spawn(
    resolve(
      `apps/zcode-cli-rust/target/debug/examples/credential_fixture${process.platform === "win32" ? ".exe" : ""}`,
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

test("Rust and Node derive the same default credential secret and read each other's ciphertext", async () => {
  const root = await mkdtemp(join(tmpdir(), "zcode-cred-cipher-"));
  const env = { ...process.env };
  delete env.ZCODE_CREDENTIAL_SECRET;
  const rust = driver(join(root, "credentials.json"), env);
  try {
    const cipher = createZCodeCredentialCipher({ env });
    const value = "tok-值-🙂";
    assert.equal(await rust.call({ op: "decrypt", value: cipher.encrypt(value) }), value);
    assert.equal(cipher.decrypt(await rust.call({ op: "encrypt", value })), value);
  } finally {
    await rust.close();
    await rm(root, { recursive: true, force: true });
  }
});

test("Rust and Node share the credential file, its path and its cross-process lock", async () => {
  const root = await mkdtemp(join(tmpdir(), "zcode-cred-store-"));
  const env: Record<string, string | undefined> = { ...process.env, ZCODE_DATA_BASE_DIR: root };
  delete env.ZCODE_CREDENTIAL_SECRET;
  const file = resolveSharedZCodeCredentialsPath({ env });
  assert.equal(file, join(root, ".zcode", "v2", "credentials.json"));
  const node = createSharedZCodeCredentialStore({ env });
  const rust = driver(file, env);
  try {
    await node.save("mcp:oauth:abc:tokens", '{"access_token":"n"}');
    assert.equal(
      await rust.call({ op: "load", key: "mcp:oauth:abc:tokens" }),
      '{"access_token":"n"}',
    );
    await rust.call({
      op: "save",
      entries: [["mcp:oauth:abc:client_information", '{"client_id":"r"}']],
    });
    assert.equal(await node.load("mcp:oauth:abc:client_information"), '{"client_id":"r"}');
    // 交错的独立 read-modify-write：锁失效时后写者会用旧快照覆盖对方的 key。
    const count = 25;
    await Promise.all([
      rust.call({ op: "burst", prefix: "rust-", count }),
      (async () => {
        for (let i = 0; i < count; i += 1) await node.save(`node-${i}`, `v${i}`);
      })(),
    ]);
    const all = JSON.parse(await readFile(file, "utf8")) as Record<string, string>;
    for (let i = 0; i < count; i += 1) {
      assert.ok(all[`rust-${i}`]?.startsWith("enc:v1:"), `rust-${i}`);
      assert.ok(all[`node-${i}`]?.startsWith("enc:v1:"), `node-${i}`);
    }
    assert.equal(await node.load("rust-7"), "v7");
    assert.equal(await rust.call({ op: "load", key: "node-7" }), "v7");
    // 锁目录在释放后不残留。
    assert.ok(!(await readdir(join(root, ".zcode", "v2"))).some((n) => n.endsWith(".lock")));
  } finally {
    await rust.close();
    await rm(root, { recursive: true, force: true });
  }
});
