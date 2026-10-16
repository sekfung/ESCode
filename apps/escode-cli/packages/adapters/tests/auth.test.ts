import { fork } from "node:child_process";
import { mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  SHARED_ESCODE_CREDENTIAL_KEYS,
  createSharedESCodeCredentialStore,
  loadSharedESCodeCredentialSync,
} from "../src/auth/index.js";

const TEST_ENV = {
  ESCODE_CREDENTIAL_SECRET: "test-secret",
};

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("shared ESCode credentials", () => {
  it("stores ZAI login credentials with escode compatible encrypted keys", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-"));

    try {
      const store = createSharedESCodeCredentialStore({
        baseDir: dir,
        env: TEST_ENV,
      });

      await store.saveZaiLoginCredentials({
        accessToken: "zai-access-token",
        jwtToken: "escode-jwt-token",
        user: {
          email: "alice@example.com",
          name: "Alice",
          user_id: "u_1",
        },
      });

      const raw = JSON.parse(await readFile(store.filePath, "utf-8")) as Record<string, string>;
      expect(store.filePath).toBe(join(dir, ".escode", "v2", "credentials.json"));
      expect(raw[SHARED_ESCODE_CREDENTIAL_KEYS.zaiAccessToken]?.startsWith("enc:v1:")).toBe(true);
      expect(await store.load(SHARED_ESCODE_CREDENTIAL_KEYS.activeProvider)).toBe("zai");
      expect(await store.load(SHARED_ESCODE_CREDENTIAL_KEYS.zaiAccessToken)).toBe(
        "zai-access-token",
      );
      expect(await store.load(SHARED_ESCODE_CREDENTIAL_KEYS.escodeJwtToken)).toBe("escode-jwt-token");
      expect(
        loadSharedESCodeCredentialSync(SHARED_ESCODE_CREDENTIAL_KEYS.zaiAccessToken, {
          baseDir: dir,
          env: TEST_ENV,
        }),
      ).toBe("zai-access-token");
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });

  it("reads legacy plaintext credential values", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-"));
    const credentialsDir = join(dir, ".escode", "v2");
    const credentialsPath = join(credentialsDir, "credentials.json");

    try {
      await mkdir(credentialsDir, { recursive: true });
      await writeFile(
        credentialsPath,
        JSON.stringify({
          [SHARED_ESCODE_CREDENTIAL_KEYS.zaiAccessToken]: "legacy-token",
        }),
      );

      const store = createSharedESCodeCredentialStore({
        baseDir: dir,
        env: TEST_ENV,
      });

      expect(await store.load(SHARED_ESCODE_CREDENTIAL_KEYS.zaiAccessToken)).toBe("legacy-token");
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });

  it("preserves every key across concurrent read-modify-write mutations", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-concurrent-"));
    const store = createSharedESCodeCredentialStore({
      baseDir: dir,
      env: TEST_ENV,
    });
    const entries = Array.from({ length: 40 }, (_, index) => [`key-${index}`, `value-${index}`]);

    try {
      await Promise.all(entries.map(([key, value]) => store.save(key!, value!)));

      await Promise.all(
        entries.map(async ([key, value]) => {
          await expect(store.load(key!)).resolves.toBe(value);
        }),
      );
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });

  it("deletes a credential only when its expected plaintext value is still current", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-compare-delete-"));
    const store = createSharedESCodeCredentialStore({
      baseDir: dir,
      env: TEST_ENV,
    });

    try {
      await store.save("oauth:snapshot", "snapshot-a");

      await expect(store.deleteIfValue("oauth:snapshot", "stale-snapshot")).resolves.toBe(false);
      await expect(store.load("oauth:snapshot")).resolves.toBe("snapshot-a");
      await expect(store.deleteIfValue("oauth:snapshot", "snapshot-a")).resolves.toBe(true);
      await expect(store.load("oauth:snapshot")).resolves.toBeNull();
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });

  it("publishes a replacement credential and removes legacy keys in one mutation", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-replace-"));
    const store = createSharedESCodeCredentialStore({
      baseDir: dir,
      env: TEST_ENV,
    });

    try {
      await store.save("oauth:legacy-client", "client-a");
      await store.save("oauth:legacy-tokens", "tokens-a");

      await store.saveReplacing("oauth:canonical", "pair-a", [
        "oauth:legacy-client",
        "oauth:legacy-tokens",
      ]);

      await expect(store.load("oauth:canonical")).resolves.toBe("pair-a");
      await expect(store.load("oauth:legacy-client")).resolves.toBeNull();
      await expect(store.load("oauth:legacy-tokens")).resolves.toBeNull();
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });

  it("preserves every key across independent Node processes", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-processes-"));
    const credentialsPath = join(dir, "credentials.json");
    const processCount = 4;
    const keysPerProcess = 8;

    try {
      await runConcurrentCredentialWorkers(credentialsPath, processCount, keysPerProcess);
      const store = createSharedESCodeCredentialStore({
        env: TEST_ENV,
        filePath: credentialsPath,
      });
      await Promise.all(
        Array.from({ length: processCount }, (_, processIndex) =>
          Array.from({ length: keysPerProcess }, async (_, keyIndex) => {
            await expect(store.load(`worker-${processIndex}:key-${keyIndex}`)).resolves.toBe(
              `value-${processIndex}-${keyIndex}`,
            );
          }),
        ).flat(),
      );
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  }, 15_000);

  it("backs up corrupt JSON and refuses to overwrite it", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-corrupt-"));
    const credentialsPath = join(dir, "credentials.json");
    const store = createSharedESCodeCredentialStore({
      env: TEST_ENV,
      filePath: credentialsPath,
    });

    try {
      await writeFile(credentialsPath, "{not-json", {
        encoding: "utf-8",
        mode: 0o644,
      });

      await Promise.all(
        Array.from({ length: 8 }, () =>
          expect(store.load("safe-key")).rejects.toThrow("Shared ESCode credentials are corrupt"),
        ),
      );
      await expect(store.save("safe-key", "safe-value")).rejects.toThrow(
        "Shared ESCode credentials are corrupt",
      );
      await expect(readFile(credentialsPath, "utf-8")).resolves.toBe("{not-json");
      const backups = (await readdir(dir)).filter((file) =>
        /^credentials\.json\.corrupt-.*\.bak$/.test(file),
      );
      expect(backups).toHaveLength(1);
      await expect(readFile(join(dir, backups[0]!), "utf-8")).resolves.toBe("{not-json");
      if (process.platform !== "win32") {
        expect((await stat(join(dir, backups[0]!))).mode & 0o777).toBe(0o600);
      }
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });

  it("deduplicates corrupt credential backups across independent Node processes", async () => {
    const dir = await mkdtemp(join(tmpdir(), "escode-auth-corrupt-processes-"));
    const credentialsPath = join(dir, "credentials.json");

    try {
      await writeFile(credentialsPath, "{not-json", "utf-8");
      await runConcurrentCredentialWorkers(credentialsPath, 6, 0, "read-corrupt");

      const backups = (await readdir(dir)).filter((file) =>
        /^credentials\.json\.corrupt-.*\.bak$/.test(file),
      );
      expect(backups).toHaveLength(1);
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });
});

async function runConcurrentCredentialWorkers(
  filePath: string,
  processCount: number,
  keysPerProcess: number,
  operation: "save" | "read-corrupt" = "save",
): Promise<void> {
  const workerUrl = new URL("./fixtures/shared-credentials-concurrent-process.ts", import.meta.url);
  const children = Array.from({ length: processCount }, (_, processIndex) =>
    fork(workerUrl, {
      env: { ...process.env, NODE_NO_WARNINGS: "1" },
      execArgv: ["--import", "tsx"],
      stdio: ["ignore", "ignore", "inherit", "ipc"],
    }),
  );

  try {
    await Promise.all(
      children.map(
        (child) =>
          new Promise<void>((resolve, reject) => {
            child.once("error", reject);
            child.once("exit", (code) =>
              reject(new Error(`credential child exited early: ${code}`)),
            );
            child.on("message", (message: { type?: unknown }) => {
              if (message.type === "ready") resolve();
            });
          }),
      ),
    );

    const results = children.map(
      (child) =>
        new Promise<void>((resolve, reject) => {
          child.once("error", reject);
          child.once("exit", (code) => {
            if (code !== 0) reject(new Error(`credential child exited with code ${code}`));
          });
          child.on("message", (message: { error?: unknown; type?: unknown }) => {
            if (message.type === "done") resolve();
            if (message.type === "error") reject(new Error(String(message.error)));
          });
        }),
    );

    children.forEach((child, processIndex) => {
      child.send({
        filePath,
        keysPerProcess,
        operation,
        processIndex,
        type: "start",
      });
    });
    await Promise.all(results);
  } finally {
    for (const child of children) {
      if (child.connected) child.disconnect();
      child.kill();
    }
  }
}
