// 知识库「导入文档」与作用域调整（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11「导入入口」「作用域调整」）。
import { createElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { V4WikiStatusResult } from "@escode/shared/escode-protocol-v4";
import { ESCodeIntlProvider } from "@/i18n/IntlProvider.js";
import { wikiEntryActions } from "@/lib/wikiGraph.js";
import {
  DEFAULT_IMPORT_SCOPE,
  appendImportPaths,
  canPickImportPaths,
  importRowTone,
  parseImportPaths,
  summarizeImportRows,
  toImportRows,
} from "@/lib/wikiImport.js";

// Radix Dialog 渲染在 portal 中，静态渲染取不到；测试里直接内联渲染内容。
vi.mock("@/components/ui/dialog.js", () => {
  const Pass = ({ children }: { children?: ReactNode }) => createElement("div", null, children);
  return {
    Dialog: ({ open, children }: { open: boolean; children?: ReactNode }) =>
      open ? createElement("div", null, children) : null,
    DialogContent: ({ children, ...props }: { children?: ReactNode }) =>
      createElement("div", props, children),
    DialogFooter: Pass,
    DialogTitle: ({ children }: { children?: ReactNode }) => createElement("h2", null, children),
  };
});

const { KnowledgeImportDialog } = await import("@/settings/KnowledgeImportDialog.js");

describe("import path helpers", () => {
  it("parses one path per line, trimming blanks and duplicates", () => {
    expect(parseImportPaths("  docs/a.md \n\n docs/b.md\r\ndocs/a.md\n")).toEqual([
      "docs/a.md",
      "docs/b.md",
    ]);
  });

  it("appends picked paths without duplicating existing lines", () => {
    expect(appendImportPaths("docs/a.md\n", ["docs/a.md", " /abs/c.docx ", ""])).toBe(
      "docs/a.md\n/abs/c.docx",
    );
  });

  it("offers native pickers only for local workspaces on platforms that return real paths", () => {
    expect(canPickImportPaths({ canSelectFilePath: true })).toBe(true);
    expect(canPickImportPaths({ canSelectFilePath: false })).toBe(false);
    expect(canPickImportPaths({ canSelectFilePath: undefined })).toBe(false);
    // 远程 workspace：本机路径对远端 runtime 无效。
    expect(canPickImportPaths({ canSelectFilePath: true, remoteSessionId: "r1" })).toBe(false);
    expect(canPickImportPaths({ canSelectFilePath: true, remoteTarget: { host: "h" } })).toBe(
      false,
    );
  });

  it("maps runtime ingest items to rows and ignores malformed items", () => {
    const rows = toImportRows([
      { path: "docs/a.md", status: "imported", created: 3, updated: 0, proposals: 0 },
      { path: "docs/legacy.doc", status: "skipped", error: "needs converter" },
      { path: "docs/x.bin", status: "weird" },
      { status: "failed" },
      null,
      "text",
    ]);
    expect(rows).toEqual([
      {
        path: "docs/a.md",
        status: "imported",
        error: undefined,
        created: 3,
        updated: 0,
        proposals: 0,
      },
      {
        path: "docs/legacy.doc",
        status: "skipped",
        error: "needs converter",
        created: 0,
        updated: 0,
        proposals: 0,
      },
      {
        path: "docs/x.bin",
        status: "unknown",
        error: undefined,
        created: 0,
        updated: 0,
        proposals: 0,
      },
    ]);
    expect(importRowTone("updated")).toBe("success");
    expect(importRowTone("degraded")).toBe("warning");
    expect(importRowTone("failed")).toBe("destructive");
    expect(importRowTone("unknown")).toBe("neutral");
  });

  it("summarizes rows for the localized result header", () => {
    const rows = toImportRows([
      { path: "a.md", status: "imported", created: 3, updated: 1, proposals: 0 },
      { path: "b.docx", status: "updated", created: 0, updated: 2, proposals: 1 },
      { path: "c.doc", status: "skipped" },
      { path: "d.pdf", status: "failed" },
    ]);
    expect(summarizeImportRows(rows)).toEqual({
      files: 4,
      created: 3,
      updated: 3,
      proposals: 1,
      failed: 1,
      skipped: 1,
    });
  });

  it("defaults the import target to the project wiki", () => {
    expect(DEFAULT_IMPORT_SCOPE).toBe("project");
  });
});

describe("scope adjustment", () => {
  it("offers promote only for live project entries, never for global ones", () => {
    expect(wikiEntryActions("project", "proposal")).toEqual(["approve", "reject", "promote"]);
    expect(wikiEntryActions("project", "approved")).toEqual(["deprecate", "promote"]);
    expect(wikiEntryActions("project", "deprecated")).toEqual([]);
    expect(wikiEntryActions("global", "approved")).toEqual(["deprecate"]);
    expect(wikiEntryActions("global", "proposal")).toEqual(["approve", "reject"]);
  });
});

const status: V4WikiStatusResult = {
  supported: true,
  enabled: true,
  scopes: [
    {
      scope: "project",
      entries: 1,
      byStatus: {},
      sources: {},
      indexVersion: 1,
      invalidFiles: 0,
      root: "/repo/.escode/wiki",
    },
    {
      scope: "global",
      entries: 0,
      byStatus: {},
      sources: {},
      indexVersion: 0,
      invalidFiles: 0,
      root: "/home/u/.escode/wiki",
    },
  ],
};

function renderDialog(canPickPaths: boolean, locale: "en-US" | "zh-CN" = "zh-CN") {
  return renderToStaticMarkup(
    createElement(
      ESCodeIntlProvider,
      { initialLocale: locale },
      createElement(KnowledgeImportDialog, {
        open: true,
        onOpenChange: () => undefined,
        status,
        canPickPaths,
        busy: false,
        onImport: async () => null,
      }),
    ),
  );
}

describe("import dialog", () => {
  it("shows both targets with their real storage locations, project selected by default", () => {
    const html = renderDialog(true);
    expect(html).toContain("/repo/.escode/wiki");
    expect(html).toContain("/home/u/.escode/wiki");
    expect(html).toMatch(
      /data-testid="knowledge-import-scope-project"[^>]*aria-checked="true"|aria-checked="true"[^>]*data-testid="knowledge-import-scope-project"/,
    );
    expect(html).toMatch(
      /aria-checked="false"[^>]*data-testid="knowledge-import-scope-global"|data-testid="knowledge-import-scope-global"[^>]*aria-checked="false"/,
    );
    expect(html).toContain("随仓库提交");
    expect(html).toContain('data-testid="knowledge-import-pick-files"');
    expect(html).toContain('data-testid="knowledge-import-pick-folder"');
    // 未输入路径时不能提交；直接批准默认关闭。
    expect(html).toMatch(
      /disabled=""[^>]*data-testid="knowledge-import-submit"|data-testid="knowledge-import-submit"[^>]*disabled=""/,
    );
    expect(html).not.toMatch(/data-testid="knowledge-import-approve"[^>]*data-state="checked"/);
  });

  it("hides native pickers when paths must be typed (remote workspace)", () => {
    const html = renderDialog(false, "en-US");
    expect(html).not.toContain("knowledge-import-pick-files");
    expect(html).toContain('data-testid="knowledge-import-paths"');
    expect(html).toContain("on the machine running this workspace");
  });
});
