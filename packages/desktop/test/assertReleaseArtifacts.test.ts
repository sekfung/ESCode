import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createPackage } from "@electron/asar";
import { describe, expect, it } from "vitest";
import {
  asarEntryPath,
  assertReleaseArtifacts,
  findNonReleaseArtifactNames,
  findSourcemapLeaks,
  hasSourceMappingUrl,
} from "../scripts/assert-release-artifacts.mjs";

// docs/desktop/release-artifact-hygiene.md：release 安装包必须是正式版，不带 sourcemap / 符号表 / 测试标记。
const repoRoot = resolve(import.meta.dirname, "../../..");

describe("release artifact hygiene", () => {
  it("rejects installers built for the test backend or with the Preview identity", () => {
    expect(
      findNonReleaseArtifactNames([
        "ESCode-3.16.0-win-x64.exe",
        "ESCode-3.16.0-win-x64_TEST.exe",
        "ESCode Preview-3.16.0-mac-arm64.dmg",
        "escode-preview_3.16.0_amd64.deb",
        "latest.yml",
        "ESCode-3.16.0-win-x64.exe.blockmap",
      ]),
    ).toEqual([
      "ESCode-3.16.0-win-x64_TEST.exe",
      "ESCode Preview-3.16.0-mac-arm64.dmg",
      "escode-preview_3.16.0_amd64.deb",
    ]);
  });

  it("flags sourcemap files and line-leading sourceMappingURL comments only", () => {
    expect(hasSourceMappingUrl("a();\n//# sourceMappingURL=main.js.map\n")).toBe(true);
    expect(hasSourceMappingUrl("a();\n/*# sourceMappingURL=main.css.map */")).toBe(true);
    // 压缩产物可能在字符串里内嵌该文本，不是 sourcemap 入口。
    expect(hasSourceMappingUrl('const s = "//# sourceMappingURL=x";')).toBe(false);
    expect(
      findSourcemapLeaks([
        { path: "app.asar/out/main/index.js", read: () => "ok();" },
        { path: "app.asar/out/main/index.js.map", read: () => "{}" },
        { path: "resources/glm/zcode.cjs", read: () => "x();\n//# sourceMappingURL=zcode.cjs.map" },
        { path: "resources/icon.png", read: () => "//# sourceMappingURL=never-read" },
      ]),
    ).toEqual([
      "app.asar/out/main/index.js.map (sourcemap file)",
      "resources/glm/zcode.cjs (sourceMappingURL)",
    ]);
  });

  it("reports asar entries with forward slashes on every platform", () => {
    // Windows 上 createPackage 以 `\` 建键、listPackage 也返回原生分隔符：报告路径须与打包平台无关。
    expect(asarEntryPath("out\\main\\index.js")).toBe("app.asar/out/main/index.js");
    expect(asarEntryPath("/out/main/index.js")).toBe("app.asar/out/main/index.js");
    expect(asarEntryPath("out/main/index.js")).toBe("app.asar/out/main/index.js");
  });

  it("builds Rust runtimes stripped in the release profile", () => {
    const cargo = readFileSync(resolve(repoRoot, "apps/escode-cli-rust/Cargo.toml"), "utf8");
    const release = /\[profile\.release\]([\s\S]*?)(?:\n\[|$)/u.exec(cargo)?.[1] ?? "";
    expect(release).toMatch(/^strip\s*=\s*true\s*$/mu);
    expect(release).not.toMatch(/^debug\s*=\s*(?!false|0|"none")/mu);
  });

  it("builds release installers for the production backend", () => {
    const workflow = readFileSync(resolve(repoRoot, ".github/workflows/release.yml"), "utf8");
    expect(workflow).toMatch(/^\s+ESCODE_ENV: production\s*$/mu);
    expect(workflow).toContain("assert-release-artifacts.mjs");
    expect(workflow).not.toMatch(/ESCODE_PREVIEW_IDENTITY:\s*"?1/u);
  });

  it("scans the packaged app.asar and resources of an electron-builder output", async () => {
    const root = await mkdtemp(join(tmpdir(), "escode-release-gate-"));
    try {
      const build = async (leaky: boolean) => {
        const dist = join(root, leaky ? "leaky" : "clean");
        const app = join(dist, "app-src");
        mkdirSync(join(app, "out", "main"), { recursive: true });
        writeFileSync(
          join(app, "out", "main", "index.js"),
          leaky ? "a();\n//# sourceMappingURL=index.js.map" : "a();",
        );
        if (leaky) writeFileSync(join(app, "out", "main", "index.js.map"), "{}");
        const resources = join(dist, "linux-unpacked", "resources");
        mkdirSync(join(resources, "glm"), { recursive: true });
        await createPackage(app, join(resources, "app.asar"));
        writeFileSync(join(resources, "glm", "zcode.cjs"), "x();");
        writeFileSync(
          join(dist, leaky ? "ESCode-1.0.0-linux-x64_TEST.deb" : "ESCode-1.0.0-linux-x64.deb"),
          "",
        );
        return dist;
      };
      const silent = { log: () => undefined };
      const clean = await build(false);
      expect(() =>
        assertReleaseArtifacts({
          distDir: clean,
          env: { ESCODE_ENV: "production" },
          logger: silent,
        }),
      ).not.toThrow();
      const leaky = await build(true);
      let message = "";
      try {
        assertReleaseArtifacts({ distDir: leaky, env: { ESCODE_ENV: "test" }, logger: silent });
      } catch (error) {
        message = (error as Error).message;
      }
      expect(message).toContain('ESCODE_ENV must be "production"');
      expect(message).toContain("non-release artifact name: ESCode-1.0.0-linux-x64_TEST.deb");
      expect(message).toContain("app.asar/out/main/index.js.map (sourcemap file)");
      expect(message).toContain("app.asar/out/main/index.js (sourceMappingURL)");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });
});
