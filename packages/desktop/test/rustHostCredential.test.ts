import { describe, expect, it } from "vitest";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, realpath, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createCredentialService, setDataBaseDir } from "@escode/services/node";
import { connectRpc, hostBinary, nextMeaningful, startHost } from "./rustHostTestSupport.js";

// docs/specs/rust-u2-credential.md §5：同一 credentials.json 上 TS 服务与 Rust Host 双向互通。

const SECRET = "rust-host-credential-test-secret";

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
    "shares one credentials.json with the TS credential service in both directions",
    async () => {
      const base = await realpath(await mkdtemp(join(tmpdir(), "escode-rust-host-cred-")));
      const home = join(base, "home");
      const dataBaseDir = join(base, "data");
      const file = join(dataBaseDir, ".escode", "v2", "credentials.json");
      const previousSecret = process.env.ESCODE_CREDENTIAL_SECRET;
      process.env.ESCODE_CREDENTIAL_SECRET = SECRET;
      setDataBaseDir(dataBaseDir);
      const ts = createCredentialService();
      const host = startHost(home, { ESCODE_CREDENTIAL_SECRET: SECRET });
      try {
        expect((await nextMeaningful(host)).kind).toBe("hostReady");
        host.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
        const listening = await nextMeaningful(host);
        const services = await connectRpc(String(listening.addr), String(listening.secret));
        const rust = services.credentialService;

        // 缺省 → null
        expect(await rust.load("missing")).toBeNull();
        expect(await ts.load("missing")).toBeNull();

        // TS 写 → Rust 读（含键裁剪）
        await ts.save("oauth:zai:access_token", "ts-token");
        expect(await rust.load("oauth:zai:access_token")).toBe("ts-token");
        expect(await rust.load("  oauth:zai:access_token  ")).toBe("ts-token");

        // Rust 写 → TS 读；覆盖写保持另一把键
        await rust.save("escodejwttoken", "rust-jwt");
        expect(await ts.load("escodejwttoken")).toBe("rust-jwt");
        await rust.save("oauth:zai:access_token", "rust-token");
        expect(await ts.load("oauth:zai:access_token")).toBe("rust-token");
        expect(await ts.load("escodejwttoken")).toBe("rust-jwt");

        // 互删；删不存在的键是无操作
        await ts.delete("escodejwttoken");
        expect(await rust.load("escodejwttoken")).toBeNull();
        await rust.delete("oauth:zai:access_token");
        expect(await ts.load("oauth:zai:access_token")).toBeNull();
        await rust.delete("never-existed");
        await ts.delete("never-existed");

        // 旧明文值原样读出
        await writeFile(file, JSON.stringify({ legacy: "plain-value" }, null, 2));
        expect(await rust.load("legacy")).toBe("plain-value");
        expect(await ts.load("legacy")).toBe("plain-value");

        // 空值：两边都能写、都读不回（密文 cipher 段为空）
        const tsEmpty = await outcome(async () => {
          await ts.save("empty", "");
          return ts.load("empty");
        });
        const rustEmpty = await outcome(async () => {
          await rust.save("empty2", "");
          return rust.load("empty2");
        });
        expect(tsEmpty.ok).toBe(false);
        expect(rustEmpty).toMatchObject({
          ok: false,
          message: (tsEmpty as { message: string }).message,
          code: (tsEmpty as { code?: string }).code,
        });

        // 密钥不匹配：解密失败的 code 与文案前缀一致
        const other = startHost(home, { ESCODE_CREDENTIAL_SECRET: "another-secret" });
        try {
          expect((await nextMeaningful(other)).kind).toBe("hostReady");
          other.send({ kind: "rpcListen", bind: "127.0.0.1:0", home, dataBaseDir });
          const otherListening = await nextMeaningful(other);
          const otherServices = await connectRpc(
            String(otherListening.addr),
            String(otherListening.secret),
          );
          await ts.save("sealed", "secret-value");
          const mismatched = await outcome(() => otherServices.credentialService.load("sealed"));
          expect(mismatched).toMatchObject({
            ok: false,
            message: "凭据解密失败：密钥不匹配或密文已损坏",
            code: "ESCODE_CREDENTIAL_DECRYPT_FAILED",
          });
        } finally {
          await other.stop();
        }

        // 损坏文件：两边都报错、都不改文件
        await mkdir(join(dataBaseDir, ".escode", "v2"), { recursive: true });
        await writeFile(file, "{ not json");
        const tsBroken = await outcome(() => ts.load("x"));
        const rustBroken = await outcome(() => rust.load("x"));
        expect(tsBroken.ok).toBe(false);
        expect(rustBroken.ok).toBe(false);
        expect(await readFile(file, "utf8")).toBe("{ not json");
        const rustWrite = await outcome(() => rust.save("x", "y"));
        expect(rustWrite.ok).toBe(false);
        expect(await readFile(file, "utf8")).toBe("{ not json");
      } finally {
        setDataBaseDir(null);
        if (previousSecret === undefined) delete process.env.ESCODE_CREDENTIAL_SECRET;
        else process.env.ESCODE_CREDENTIAL_SECRET = previousSecret;
        await host.stop();
        await rm(base, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
