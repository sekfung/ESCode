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
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { REMOTE_AGENT_OFFICIAL_PLUGIN_REQUIRED_RELATIVE_PATHS } from "@escode/server/remote/escodeAgentOfficialPluginAssets.js";
import { REQUIRED_SERVER_BUNDLE_MARKERS } from "@escode/server/remote/serverBundleDeployCheck.js";
import { packSourceAsDeterministicTarGzip } from "../../../scripts/deterministic-tar-archive.mjs";
import { getBuildMetadata } from "../scripts/build-metadata.mjs";
import { getTargetPlatform } from "../scripts/target-platform.mjs";

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scriptPath = join(desktopRoot, "scripts", "prepare-bundled-remote-assets.ts");
const version = getBuildMetadata().appVersion;
// 本地源由同一仓库版本构建：manifest 版本与 App 版本不一致说明 mock-cdn 残留了其他版本。
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
  {
    id: "server-bundle",
    mount: "server",
    files: { "zcode-server.cjs": `server ${REQUIRED_SERVER_BUNDLE_MARKERS.join(" ")}` },
  },
  { id: "node-runtime", mount: `node/${remotePlatform}`, files: { node: "node" } },
  { id: "node-pty", mount: `node-pty/${remotePlatform}`, files: { "pty.node": "pty" } },
  {
    id: "glm",
    mount: `glm/${remotePlatform}`,
    files: {
      "zcode.cjs": "agent",
      ...Object.fromEntries(
        REMOTE_AGENT_OFFICIAL_PLUGIN_REQUIRED_RELATIVE_PATHS.map((relativePath) => [
          `packages/${relativePath}`,
          "{}",
        ]),
      ),
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

interface FakeMockCdnOptions {
  omitComponents?: string[];
  corruptShaFor?: string;
  skipManifest?: boolean;
  manifestAppVersion?: string;
  /** 覆盖某组件的文件，模拟上游制品（.zcode-plugin 布局、旧能力标记）。 */
  overrideFiles?: Record<string, Record<string, string>>;
}

interface FakeMockCdn {
  dir: string;
  close: () => void;
}

/** 按 prepare-prebuilds 的输出布局造一份 mock-cdn：releases/<version>/manifest + 根目录下的组件归档。 */
function createFakeMockCdn(options: FakeMockCdnOptions = {}): FakeMockCdn {
  const root = mkdtempSync(join(tmpdir(), "escode-fake-mock-cdn-"));
  const dir = join(root, "mock-cdn");
  const included = FIXTURE_COMPONENTS.filter(
    (component) => !options.omitComponents?.includes(component.id),
  );

  const components: Array<Record<string, string>> = [];
  for (const component of included) {
    const sourceDir = join(root, "src", component.id);
    const files = options.overrideFiles?.[component.id] ?? component.files;
    for (const [relativePath, content] of Object.entries(files)) {
      const targetPath = join(sourceDir, ...relativePath.split("/"));
      mkdirSync(dirname(targetPath), { recursive: true });
      writeFileSync(targetPath, content);
    }
    const archivePath = join(dir, artifactPathFor(component));
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
    const manifestPath = join(dir, "releases", version, `manifest-${remotePlatform}.json`);
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

  return { dir, close: () => rmSync(root, { recursive: true, force: true }) };
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

function runPrepareScript(sourceDir: string): Promise<{ status: number | null; stderr: string }> {
  const env = { ...process.env, ESCODE_BUNDLED_REMOTE_ASSETS_SOURCE_DIR: sourceDir };
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
  it("从本地 mock-cdn 取当前版本制品、校验 sha256 并组装成运行时可直接消费的 release 目录", async () => {
    const mockCdn = createFakeMockCdn();
    try {
      const result = await runPrepareScript(mockCdn.dir);

      expect(result.status).toBe(0);
      // 远程制品名沿用发布侧约定：server/zcode-server.cjs、glm/<arch>/zcode.cjs。
      expect(existsSync(join(releaseDir, "server", "zcode-server.cjs"))).toBe(true);
      expect(existsSync(join(releaseDir, "glm", remotePlatform, "zcode.cjs"))).toBe(true);
      expect(existsSync(join(releaseDir, "node", remotePlatform, "node"))).toBe(true);
      expect(existsSync(join(releaseDir, "node-pty", remotePlatform, "pty.node"))).toBe(true);
      expect(existsSync(join(releaseDir, "tools", remotePlatform, "bfs", "bfs"))).toBe(true);
      expect(existsSync(join(releaseDir, "tools", remotePlatform, "ripgrep", "rg"))).toBe(true);
      expect(existsSync(join(releaseDir, "tools", remotePlatform, "ugrep", "ugrep"))).toBe(true);
      expect(existsSync(join(releaseDir, `manifest-${remotePlatform}.json`))).toBe(true);
      const stagedManifest = JSON.parse(
        readFileSync(join(releaseDir, `manifest-${remotePlatform}.json`), "utf8"),
      ) as { appVersion: string };
      expect(stagedManifest.appVersion).toBe(version);
      // 非目标平台不得进包。
      expect(existsSync(join(releaseDir, "node", otherPlatform))).toBe(false);
    } finally {
      mockCdn.close();
    }
  });

  it("制品 sha256 与清单不一致时构建失败", async () => {
    const mockCdn = createFakeMockCdn({ corruptShaFor: "glm" });
    try {
      const result = await runPrepareScript(mockCdn.dir);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("sha256 mismatch");
    } finally {
      mockCdn.close();
    }
  });

  it("清单缺少必需组件时构建失败", async () => {
    const mockCdn = createFakeMockCdn({ omitComponents: ["ugrep"] });
    try {
      const result = await runPrepareScript(mockCdn.dir);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("staged release incomplete");
    } finally {
      mockCdn.close();
    }
  });

  // 回归：上游 ZCode 制品的官方插件是 .zcode-plugin 布局，运行时按 .escode-plugin 判定 glm 不完整并回退 CDN。
  it("glm 官方插件不符合运行时必需路径（上游 .zcode-plugin 布局）时构建失败", async () => {
    const mockCdn = createFakeMockCdn({
      overrideFiles: {
        glm: {
          "zcode.cjs": "agent",
          "packages/skill-creator-plugin/.zcode-plugin/plugin.json": "{}",
        },
      },
    });
    try {
      const result = await runPrepareScript(mockCdn.dir);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("staged release incomplete");
      expect(result.stderr).toContain(".escode-plugin");
    } finally {
      mockCdn.close();
    }
  });

  // 回归：上游 server bundle 的能力标记是 __zcode_ 前缀，远端检查每次都判定缺标记并重传 server。
  it("server bundle 缺少远端检查要求的能力标记时构建失败", async () => {
    const mockCdn = createFakeMockCdn({
      overrideFiles: {
        "server-bundle": { "zcode-server.cjs": "server __zcode_rpc_nested_uint8array_v1" },
      },
    });
    try {
      const result = await runPrepareScript(mockCdn.dir);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("server bundle missing required markers");
    } finally {
      mockCdn.close();
    }
  });

  it("manifest 版本与 App 版本不一致时构建失败", async () => {
    const mockCdn = createFakeMockCdn({ manifestAppVersion: mismatchedVersion });
    try {
      const result = await runPrepareScript(mockCdn.dir);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("manifest identity mismatch");
    } finally {
      mockCdn.close();
    }
  });

  it("本地 mock-cdn 尚未构建该版本时给出可诊断的失败", async () => {
    const mockCdn = createFakeMockCdn({ skipManifest: true });
    try {
      const result = await runPrepareScript(mockCdn.dir);

      expect(result.status).not.toBe(0);
      expect(result.stderr).toContain("prepare:remote-assets");
    } finally {
      mockCdn.close();
    }
  });
});
