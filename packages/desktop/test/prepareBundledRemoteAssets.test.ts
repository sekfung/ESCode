import { afterAll, describe, expect, it } from "vitest";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { packSourceAsDeterministicTarGzip } from "../../../scripts/deterministic-tar-archive.mjs";
import { getBuildMetadata } from "../scripts/build-metadata.mjs";
import { getTargetPlatform } from "../scripts/target-platform.mjs";

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scriptPath = join(desktopRoot, "scripts", "prepare-bundled-remote-assets.ts");
const version = getBuildMetadata().appVersion;
// 只维护一个版本号：CDN 上的制品版本必须等于 App 版本，不一致时构建应失败。
const mismatchedVersion = "9.9.9-mismatch-test";
const remotePlatform = `linux-${getTargetPlatform().arch}`;
const otherPlatform = remotePlatform === "linux-x64" ? "linux-arm64" : "linux-x64";

const bundledDir = join(desktopRoot, "bundled-remote-assets");
const releaseDir = join(bundledDir, "releases", version);

interface FixtureComponent {
  id: string;
  mount: string;
  files: Record<string, string>;
}

/** 与运行时必需条目对齐：断言脚本产出的布局可被直接消费。 */
const FIXTURE_COMPONENTS: FixtureComponent[] = [
  { id: "server-bundle", mount: "server", files: { "zcode-server.cjs": "server" } },
  { id: "node-runtime", mount: `node/${remotePlatform}`, files: { node: "node" } },
  { id: "node-pty", mount: `node-pty/${remotePlatform}`, files: { "pty.node": "pty" } },
  {
    id: "glm",
    mount: `glm/${remotePlatform}`,
    files: {
      "zcode.cjs": "agent",
      "packages/skill-creator-plugin/.escode-plugin/plugin.json": "{}",
    },
  },
  { id: "bfs", mount: `tools/${remotePlatform}/bfs`, files: { bfs: "bfs" } },
  { id: "ripgrep", mount: `tools/${remotePlatform}/ripgrep`, files: { rg: "rg" } },
  { id: "ugrep", mount: `tools/${remotePlatform}/ugrep`, files: { ugrep: "ugrep" } },
];

/** 制品版本带 `+`（真实清单即 v0.13.3+04947e21cce0），用于锁定 %2B 编码行为。 */
function componentArtifactVersion(id: string): string {
  return `v1.0.0+${id.replace(/[^a-z]/g, "").slice(0, 6)}`;
}

function artifactPathFor(component: FixtureComponent): string {
  return `components/${remotePlatform}/${component.id}/${componentArtifactVersion(component.id)}.tar.gz`;
}

interface FakeCdnOptions {
  omitComponents?: string[];
  corruptShaFor?: string;
  skipManifest?: boolean;
}

interface FakeCdn {
  baseUrl: string;
  close: () => Promise<void>;
}

async function startFakeCdn(options: FakeCdnOptions = {}): Promise<FakeCdn> {
  const root = mkdtempSync(join(tmpdir(), "escode-fake-cdn-"));
  const included = FIXTURE_COMPONENTS.filter(
    (component) => !options.omitComponents?.includes(component.id),
  );

  const components: Array<Record<string, string>> = [];
  for (const component of included) {
    const sourceDir = join(root, "src", component.id);
    for (const [relativePath, content] of Object.entries(component.files)) {
      const targetPath = join(sourceDir, ...relativePath.split("/"));
      mkdirSync(dirname(targetPath), { recursive: true });
      writeFileSync(targetPath, content);
    }
    const archivePath = join(root, "serve", artifactPathFor(component));
    mkdirSync(dirname(archivePath), { recursive: true });
    packSourceAsDeterministicTarGzip(sourceDir, archivePath);

    components.push({
      id: component.id,
      version: componentArtifactVersion(component.id),
      sha256:
        component.id === options.corruptShaFor
          ? "0".repeat(64)
          : createHash("sha256").update(readFileSync(archivePath)).digest("hex"),
      artifactPath: artifactPathFor(component),
      mount: component.mount,
    });
  }

  if (!options.skipManifest) {
    const manifestPath = join(root, "serve", version, `manifest-${remotePlatform}.json`);
    mkdirSync(dirname(manifestPath), { recursive: true });
    writeFileSync(
      manifestPath,
      JSON.stringify({
        schemaVersion: 1,
        appVersion: options.manifestAppVersion ?? version,
        platformArch: remotePlatform,
        components,
      }),
    );
  }

  const serveRoot = join(root, "serve");
  const server: Server = createServer((request, response) => {
    const pathname = decodeURIComponent(new URL(request.url ?? "/", "http://localhost").pathname);
    const filePath = join(serveRoot, ...pathname.split("/").filter(Boolean));
    // 未内置的平台（例如 darwin）在这里必然 404，与真实 CDN 行为一致。
    if (!filePath.startsWith(serveRoot) || !existsSync(filePath)) {
      response.writeHead(404).end("not found");
      return;
    }
    response.writeHead(200).end(readFileSync(filePath));
  });
  await new Promise<void>((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;

  return {
    baseUrl: `http://127.0.0.1:${port}`,
    close: async () => {
      await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
      rmSync(root, { recursive: true, force: true });
    },
  };
}

// 脚本按 packages/desktop 相对路径解析输出目录，只能就地验证；沿用备份挪走 + 结束还原的约定。
const bundledBackupDir = existsSync(bundledDir) ? `${bundledDir}.test-backup` : null;
if (bundledBackupDir) {
  rmSync(bundledBackupDir, { recursive: true, force: true });
  renameSync(bundledDir, bundledBackupDir);
}

afterAll(() => {
  rmSync(bundledDir, { recursive: true, force: true });
  if (bundledBackupDir) {
    renameSync(bundledBackupDir, bundledDir);
  }
});

/**
 * 必须用异步 spawn：夹具 CDN 跑在本进程的 HTTP server 上，
 * spawnSync 会阻塞事件循环，导致子进程的请求永远等不到响应而死锁。
 */
function runPrepareScript(baseUrl: string): Promise<{ status: number | null; stderr: string }> {
  const env = { ...process.env };
  // 走同一优先级里的显式覆盖，避免 CI 注入的 CDN_DOMAIN 干扰夹具。
  delete env["CDN_DOMAIN"];
  delete env["OSS_PATH_PREFIX"];
  env["ESCODE_REMOTE_ASSET_CDN_BASE_URL"] = baseUrl;

  return new Promise((resolvePromise) => {
    const child = spawn(process.execPath, ["--import", "tsx", scriptPath], {
      cwd: desktopRoot,
      env,
    });
    let stderr = "";
    child.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    child.on("close", (status) => resolvePromise({ status, stderr }));
  });
}

describe("prepare-bundled-remote-assets", () => {
  it("从 CDN 下载当前版本制品、校验 sha256 并组装成运行时可直接消费的 release 目录", async () => {
    const cdn = await startFakeCdn();
    try {
      const result = await runPrepareScript(cdn.baseUrl);

      expect(result.status).toBe(0);
      // 制品名必须是上游名：server/zcode-server.cjs、glm/<arch>/zcode.cjs。
      expect(existsSync(join(releaseDir, "server", "zcode-server.cjs"))).toBe(true);
      expect(existsSync(join(releaseDir, "glm", remotePlatform, "zcode.cjs"))).toBe(true);
      expect(existsSync(join(releaseDir, "node", remotePlatform, "node"))).toBe(true);
      expect(existsSync(join(releaseDir, "node-pty", remotePlatform, "pty.node"))).toBe(true);
      expect(existsSync(join(releaseDir, "tools", remotePlatform, "bfs", "bfs"))).toBe(true);
      expect(existsSync(join(releaseDir, "tools", remotePlatform, "ripgrep", "rg"))).toBe(true);
      expect(existsSync(join(releaseDir, "tools", remotePlatform, "ugrep", "ugrep"))).toBe(true);
      expect(existsSync(join(releaseDir, `manifest-${remotePlatform}.json`))).toBe(true);
      // 落盘目录用 App 版本（运行时按 ESCODE_VERSION 找），内容来自上游源版本。
      const stagedManifest = JSON.parse(
        readFileSync(join(releaseDir, `manifest-${remotePlatform}.json`), "utf8"),
      ) as { appVersion: string };
      expect(stagedManifest.appVersion).toBe(version);
      // 非目标平台不得进包。
      expect(existsSync(join(releaseDir, "node", otherPlatform))).toBe(false);
    } finally {
      await cdn.close();
    }
  });

  it("制品 sha256 与清单不一致时构建失败", async () => {
    const cdn = await startFakeCdn({ corruptShaFor: "glm" });
    try {
      const result = await runPrepareScript(cdn.baseUrl);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("sha256 mismatch");
    } finally {
      await cdn.close();
    }
  });

  it("清单缺少必需组件时构建失败", async () => {
    const cdn = await startFakeCdn({ omitComponents: ["ugrep"] });
    try {
      const result = await runPrepareScript(cdn.baseUrl);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("staged release incomplete");
    } finally {
      await cdn.close();
    }
  });

  it("CDN 上的 manifest 版本与 App 版本不一致时构建失败", async () => {
    const cdn = await startFakeCdn({ manifestAppVersion: mismatchedVersion });
    try {
      const result = await runPrepareScript(cdn.baseUrl);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("manifest identity mismatch");
    } finally {
      await cdn.close();
    }
  });

  it("该版本制品尚未发布时按候选给出可诊断的失败", async () => {
    const cdn = await startFakeCdn({ skipManifest: true });
    try {
      const result = await runPrepareScript(cdn.baseUrl);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("未找到");
    } finally {
      await cdn.close();
    }
  });
});
