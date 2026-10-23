import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-credential.md §5：同一 credentials.json 上的 TS 加密观察已冻结为仓内资产
// （packages/desktop/test/fixtures/golden/rustHostCredential.*）。测试不再实例化 TS credential 服务，
// 直接用 TS 写出的密文夹具验证 Rust Host 的解密/加密/错误语义。
const goldenFile = new URL("./fixtures/golden/rustHostCredential.oracle.json", import.meta.url);
const seedFile = new URL("./fixtures/golden/rustHostCredential.seed.json", import.meta.url);

const SECRET = "rust-host-credential-test-secret";
const ENCRYPTED_PREFIX = "enc:v1:";

async function outcome<T>(run: () => Promise<T>) {
  try {
    return { ok: true as const, value: await run() };
  } catch (error) {
    const e = error as { message?: string; code?: string };
    return { ok: false as const, message: e.message ?? String(error), code: e.code };
  }
}

describe("rust host credential", () => {
  it.skipIf(!existsSync(hostBinary))(
    "matches the frozen TS credential observations with a shared credentials.json",
    async () => {
      const golden = JSON.parse(await readFile(goldenFile, "utf8")) as {
        observations: {
          missing: null;
          tsEncryptedReads: Record<string, string>;
          emptyValue: { ok: false; message: string; code: string };
          wrongSecret: { ok: false; message: string; code: string };
          corrupt: { ok: true; value: { loadOk: boolean; saveOk: boolean } };
        };
      };
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-cred-")));
      const home = join(base, "home");
      const dataBaseDir = join(base, "data");
      const file = join(dataBaseDir, ".escode", "v2", "credentials.json");
      await mkdir(join(dataBaseDir, ".escode", "v2"), { recursive: true });
      // TS 生成的密文夹具：证明 Rust 能解密 TS 写出的 AES-GCM 值。
      await copyFile(seedFile, file);
      const host = startHost(home, { ESCODE_CREDENTIAL_SECRET: SECRET });
      const wrongSecretHost = startHost(home, { ESCODE_CREDENTIAL_SECRET: "another-secret" });
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const rust = services.credentialService;

        // 缺省 → null
        expect(await rust.load("missing")).toBe(golden.observations.missing);

        // TS 写 → Rust 读（含键裁剪）；旧明文值原样读出。
        expect(await rust.load("oauth:zai:access_token")).toBe(
          golden.observations.tsEncryptedReads["oauth:zai:access_token"],
        );
        expect(await rust.load("  oauth:zai:access_token  ")).toBe(
          golden.observations.tsEncryptedReads["oauth:zai:access_token"],
        );
        expect(await rust.load("escodejwttoken")).toBe(
          golden.observations.tsEncryptedReads.escodejwttoken,
        );
        expect(await rust.load("legacy")).toBe(golden.observations.tsEncryptedReads.legacy);

        // Rust 写 → 读回；覆盖写保持另一把键。
        await rust.save("escodejwttoken", "rust-jwt");
        expect(await rust.load("escodejwttoken")).toBe("rust-jwt");
        await rust.save("oauth:zai:access_token", "rust-token");
        expect(await rust.load("oauth:zai:access_token")).toBe("rust-token");
        expect(await rust.load("escodejwttoken")).toBe("rust-jwt");

        // Rust 写出的值必须仍是加密存储，且旧的 TS/明文夹具保持可读。
        const onDisk = JSON.parse(await readFile(file, "utf8")) as Record<string, string>;
        expect(onDisk.escodejwttoken?.startsWith(ENCRYPTED_PREFIX)).toBe(true);
        expect(onDisk["oauth:zai:access_token"]?.startsWith(ENCRYPTED_PREFIX)).toBe(true);
        expect(onDisk.sealed).toBe(
          (JSON.parse(await readFile(seedFile, "utf8")) as Record<string, string>).sealed,
        );
        expect(onDisk.legacy).toBe(golden.observations.tsEncryptedReads.legacy);

        // 互删；删不存在的键是无操作。
        await rust.delete("escodejwttoken");
        expect(await rust.load("escodejwttoken")).toBeNull();
        await rust.delete("oauth:zai:access_token");
        expect(await rust.load("oauth:zai:access_token")).toBeNull();
        await rust.delete("never-existed");

        // 空值：保存成功但读回失败（密文 cipher 段为空）。
        const rustEmpty = await outcome(async () => {
          await rust.save("empty2", "");
          return rust.load("empty2");
        });
        expect(rustEmpty).toMatchObject(golden.observations.emptyValue);

        // 密钥不匹配：解密失败的 code 与文案前缀一致。
        expect((await nextMeaningful(wrongSecretHost)).kind).toBe("hostReady");
        wrongSecretHost.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const wrongSecretListening = await nextMeaningful(wrongSecretHost);
        const wrongSecretServices = await connectRpc(
          String(wrongSecretListening.addr),
          String(wrongSecretListening.secret),
        );
        const mismatched = await outcome(() =>
          wrongSecretServices.credentialService.load("sealed"),
        );
        expect(mismatched).toMatchObject(golden.observations.wrongSecret);

        // 损坏文件：两边都报错、都不改文件。
        await writeFile(file, "{ not json");
        const rustBroken = await outcome(() => rust.load("x"));
        expect(rustBroken.ok).toBe(golden.observations.corrupt.value.loadOk);
        expect(await readFile(file, "utf8")).toBe("{ not json");
        const rustWrite = await outcome(() => rust.save("x", "y"));
        expect(rustWrite.ok).toBe(golden.observations.corrupt.value.saveOk);
        expect(await readFile(file, "utf8")).toBe("{ not json");
      } finally {
        await wrongSecretHost.stop();
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
