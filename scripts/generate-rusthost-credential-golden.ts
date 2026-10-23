// Phase 3 golden freeze：rustHostCredential.test.ts 的 TS `createCredentialService` 观察录制。
// 一次性脚本；删除 TS 实现前用于重录。`--check` 校验仓内 golden 与当前 TS 观察一致。
//
// 录制的固定观察：
// - `seed`：TS 用固定密钥加密写出的 credentials.json（提交为独立夹具，测试时复制进数据目录）；
// - `observations`：缺省读、TS 密文/明文读数、空值错误、错误密钥错误与损坏文件错误。
// 密文含随机 IV，因此 oracle JSON 只存确定性观察；seed 夹具在 `--check` 时用 TS 解密复验。
import { deepEqual } from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setDataBaseDir } from "../packages/services/src/node.ts";

const SECRET = "rust-host-credential-test-secret";
const WRONG_SECRET = "another-secret";
const ENCRYPTED_PREFIX = "enc:v1:";

async function outcome<T>(run: () => Promise<T>) {
  try {
    return { ok: true as const, value: JSON.parse(JSON.stringify((await run()) ?? null)) };
  } catch (error) {
    const e = error as { message?: string; code?: string };
    return { ok: false as const, message: e.message ?? String(error), code: e.code };
  }
}

const base = await mkdtemp(join(tmpdir(), "rusthost-credential-golden-"));
const dataBaseDir = join(base, "data");
const emptyDir = join(base, "empty");
const corruptDir = join(base, "corrupt");
const previousSecret = process.env.ESCODE_CREDENTIAL_SECRET;
process.env.ESCODE_CREDENTIAL_SECRET = SECRET;

try {
  const { createCredentialService } = await import("../packages/services/src/node.ts");
  setDataBaseDir(dataBaseDir);
  const ts = createCredentialService();
  const file = join(dataBaseDir, ".escode", "v2", "credentials.json");
  await mkdir(join(dataBaseDir, ".escode", "v2"), { recursive: true });

  // TS 写出的密文夹具（Rust 读取）与旧明文值。
  await ts.save("oauth:zai:access_token", "ts-token");
  await ts.save("escodejwttoken", "rust-jwt");
  await ts.save("sealed", "secret-value");
  const seeded = JSON.parse(await readFile(file, "utf8")) as Record<string, string>;
  seeded.legacy = "plain-value";
  await writeFile(file, `${JSON.stringify(seeded, null, 2)}\n`);

  const observations = {
    missing: await ts.load("missing"),
    tsEncryptedReads: {
      "oauth:zai:access_token": await ts.load("oauth:zai:access_token"),
      escodejwttoken: await ts.load("escodejwttoken"),
      sealed: await ts.load("sealed"),
      legacy: await ts.load("legacy"),
    },
    emptyValue: await outcome(async () => {
      setDataBaseDir(emptyDir);
      const isolated = createCredentialService();
      await isolated.save("empty", "");
      return isolated.load("empty");
    }),
    wrongSecret: await outcome(async () => {
      setDataBaseDir(dataBaseDir);
      process.env.ESCODE_CREDENTIAL_SECRET = WRONG_SECRET;
      const mismatched = createCredentialService();
      return mismatched.load("sealed");
    }),
    corrupt: await outcome(async () => {
      setDataBaseDir(corruptDir);
      const isolated = createCredentialService();
      await mkdir(join(corruptDir, ".escode", "v2"), { recursive: true });
      const brokenFile = join(corruptDir, ".escode", "v2", "credentials.json");
      await writeFile(brokenFile, "{ not json");
      const load = await outcome(() => isolated.load("x"));
      const save = await outcome(() => isolated.save("x", "y"));
      // 错误文案含绝对路径；原始差分只比较失败与否。
      return { loadOk: load.ok, saveOk: save.ok };
    }),
  };
  process.env.ESCODE_CREDENTIAL_SECRET = SECRET;

  // 确认 seed 中实际是加密值（不是明文），否则夹具失去跨语言解密意义。
  if (!String(seeded["oauth:zai:access_token"]).startsWith(ENCRYPTED_PREFIX)) {
    throw new Error("seed oauth token was not encrypted");
  }
  for (const key of ["oauth:zai:access_token", "escodejwttoken", "sealed"]) {
    if (!String(seeded[key]).startsWith(ENCRYPTED_PREFIX)) {
      throw new Error(`seed key ${key} was not encrypted`);
    }
  }
  if (seeded.legacy !== "plain-value") {
    throw new Error("seed legacy value must stay plaintext");
  }

  const golden = {
    _readme:
      "rustHostCredential.test.ts 的 TS createCredentialService 观察冻结（Phase 3 golden freeze）。密文含随机 IV，只有确定性观察写入本文件；TS 加密夹具见 rustHostCredential.seed.json，由生成器在 --check 时用 TS 解密复验。",
    observations,
  };

  const goldenDir = "packages/desktop/test/fixtures/golden";
  const jsonPath = join(goldenDir, "rustHostCredential.oracle.json");
  const seedPath = join(goldenDir, "rustHostCredential.seed.json");
  const json = `${JSON.stringify(golden, null, 2)}\n`;
  if (process.argv.includes("--check")) {
    deepEqual(
      JSON.parse(await readFile(jsonPath, "utf8")),
      JSON.parse(json),
      "rustHostCredential golden 与当前 TS 观察漂移",
    );
    // seed 由 TS 生成一次；用当前 TS 解密复验，避免夹具被误改后静默失效。
    setDataBaseDir(join(base, "check-seed"));
    await mkdir(join(base, "check-seed", ".escode", "v2"), { recursive: true });
    await copyFile(seedPath, join(base, "check-seed", ".escode", "v2", "credentials.json"));
    const checker = createCredentialService();
    const checkedSeed = {
      "oauth:zai:access_token": await checker.load("oauth:zai:access_token"),
      escodejwttoken: await checker.load("escodejwttoken"),
      sealed: await checker.load("sealed"),
      legacy: await checker.load("legacy"),
    };
    deepEqual(checkedSeed, observations.tsEncryptedReads, "rustHostCredential seed 解密漂移");
    console.log("rustHostCredential golden: OK");
  } else {
    await mkdir(goldenDir, { recursive: true });
    await writeFile(jsonPath, json);
    await copyFile(file, seedPath);
    console.log("wrote", jsonPath, seedPath, "base=", base);
  }
} finally {
  setDataBaseDir(null);
  if (previousSecret === undefined) delete process.env.ESCODE_CREDENTIAL_SECRET;
  else process.env.ESCODE_CREDENTIAL_SECRET = previousSecret;
  await rm(base, { recursive: true, force: true });
}
