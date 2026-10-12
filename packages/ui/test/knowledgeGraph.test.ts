// 知识图谱页（apps/escode-cli-rust/docs/llm-wiki/01-spec.md §11）：布局、阶段判定、筛选参数、详情与治理操作呈现。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type {
  V4WikiEntryResult,
  V4WikiGraphNode,
  V4WikiStatusResult,
} from "@escode/shared/escode-protocol-v4";
import { ESCodeIntlProvider } from "@/i18n/IntlProvider.js";
import {
  DEFAULT_WIKI_GRAPH_FILTERS,
  WIKI_GRAPH_DEFAULT_LIMIT,
  buildWikiGraphQuery,
  resolveWikiPhase,
} from "@/hooks/useWikiKnowledge.js";
import {
  layoutWikiGraph,
  wikiEdgeStyle,
  wikiKindTone,
  wikiNeighborhood,
  wikiNodeDegrees,
  wikiReviewActions,
  wikiStatusPresentation,
} from "@/lib/wikiGraph.js";
import { KnowledgeEntryDetail } from "@/settings/KnowledgeEntryDetail.js";
import { KnowledgePhaseNotice } from "@/settings/KnowledgeSettingsSection.js";
import { createSettingsPageConfig } from "@/settings/settingsPageConfig.js";

const ids = (count: number) => Array.from({ length: count }, (_, index) => ({ id: `n${index}` }));

function distance(a: { x: number; y: number }, b: { x: number; y: number }) {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

describe("layoutWikiGraph", () => {
  it("is deterministic for the same nodes and edges", () => {
    const nodes = ids(12);
    const edges = [
      { from: "n0", to: "n1" },
      { from: "n1", to: "n2" },
    ];
    const first = layoutWikiGraph(nodes, edges);
    const second = layoutWikiGraph(nodes, edges);
    for (const node of nodes) {
      expect(second.get(node.id)).toEqual(first.get(node.id));
    }
  });

  it("keeps every node inside the canvas and separates coincident nodes", () => {
    const nodes = ids(30);
    // 只验证力导向阶段的边界：关闭标签防重叠（它允许越出画布，由 fitView 适配）。
    const positions = layoutWikiGraph(nodes, [], {
      width: 800,
      height: 600,
      labelWidth: 0,
      labelHeight: 0,
    });
    for (const point of positions.values()) {
      expect(Number.isFinite(point.x) && Number.isFinite(point.y)).toBe(true);
      expect(point.x).toBeGreaterThanOrEqual(0);
      expect(point.x).toBeLessThanOrEqual(800);
      expect(point.y).toBeGreaterThanOrEqual(0);
      expect(point.y).toBeLessThanOrEqual(600);
    }
    const unique = new Set(
      [...positions.values()].map((p) => `${p.x.toFixed(1)}:${p.y.toFixed(1)}`),
    );
    expect(unique.size).toBe(nodes.length);
  });

  it("pulls linked nodes closer than the average unlinked pair", () => {
    const nodes = ids(16);
    const edges = [
      { from: "n0", to: "n1" },
      { from: "n2", to: "n3" },
      { from: "n4", to: "n5" },
    ];
    const positions = layoutWikiGraph(nodes, edges);
    const linked =
      edges.reduce((sum, e) => sum + distance(positions.get(e.from)!, positions.get(e.to)!), 0) /
      edges.length;
    let total = 0;
    let pairs = 0;
    for (let i = 6; i < 16; i += 1) {
      for (let j = i + 1; j < 16; j += 1) {
        total += distance(positions.get(`n${i}`)!, positions.get(`n${j}`)!);
        pairs += 1;
      }
    }
    expect(linked).toBeLessThan(total / pairs);
  });

  it("separates label boxes in a dense star around one source", () => {
    const nodes = [{ id: "source" }, ...ids(14)];
    const edges = ids(14).map((node) => ({ from: node.id, to: "source" }));
    const positions = [...layoutWikiGraph(nodes, edges).values()];
    for (let i = 0; i < positions.length; i += 1) {
      for (let j = i + 1; j < positions.length; j += 1) {
        const overlapX = 120 - Math.abs(positions[i]!.x - positions[j]!.x);
        const overlapY = 36 - Math.abs(positions[i]!.y - positions[j]!.y);
        expect(overlapX <= 0.5 || overlapY <= 0.5).toBe(true);
      }
    }
  });

  it("ignores edges that reference unknown nodes or self loops", () => {
    const positions = layoutWikiGraph(ids(2), [
      { from: "n0", to: "missing" },
      { from: "n1", to: "n1" },
    ]);
    expect(positions.size).toBe(2);
  });
});

describe("graph presentation rules", () => {
  it("maps kinds to tones and statuses to border/fade", () => {
    expect(wikiKindTone("standard")).toBe("brand");
    expect(wikiKindTone("adr")).toBe("blue");
    expect(wikiKindTone("anti-pattern")).toBe("destructive");
    expect(wikiKindTone(undefined)).toBe("neutral");
    expect(wikiStatusPresentation("proposal")).toEqual({ border: "dashed", faded: false });
    expect(wikiStatusPresentation("needs-review").border).toBe("warning");
    expect(wikiStatusPresentation("deprecated").faded).toBe(true);
    expect(wikiStatusPresentation("approved")).toEqual({ border: "solid", faded: false });
    expect(wikiEdgeStyle("conflict").stroke).toContain("destructive");
    expect(wikiEdgeStyle("supersedes").strokeDasharray).toBeDefined();
  });

  it("only offers review actions that the runtime accepts for the status", () => {
    expect(wikiReviewActions("proposal")).toEqual(["approve", "reject"]);
    expect(wikiReviewActions("approved")).toEqual(["deprecate"]);
    expect(wikiReviewActions("needs-review")).toEqual(["approve", "deprecate"]);
    for (const retired of ["deprecated", "superseded", "rejected"] as const) {
      expect(wikiReviewActions(retired)).toEqual([]);
    }
  });

  it("computes degrees and one-hop neighbourhood", () => {
    const edges = [
      { from: "a", to: "b" },
      { from: "b", to: "c" },
    ];
    expect(wikiNodeDegrees(edges).get("b")).toBe(2);
    expect([...wikiNeighborhood("a", edges)].sort()).toEqual(["a", "b"]);
    expect(wikiNeighborhood(null, edges).size).toBe(0);
  });
});

describe("knowledge data rules", () => {
  it("resolves the page phase from the runtime status", () => {
    expect(resolveWikiPhase({ supported: false, enabled: false })).toBe("unsupported");
    expect(resolveWikiPhase({ supported: true, enabled: false })).toBe("disabled");
    expect(resolveWikiPhase({ supported: true, enabled: true })).toBe("ready");
  });

  it("omits empty filters and always bounds the node count", () => {
    expect(buildWikiGraphQuery(DEFAULT_WIKI_GRAPH_FILTERS)).toEqual({
      includeSources: true,
      limit: WIKI_GRAPH_DEFAULT_LIMIT,
    });
    expect(
      buildWikiGraphQuery({
        scopes: ["project"],
        statuses: ["approved"],
        kinds: ["adr"],
        includeSources: false,
      }),
    ).toEqual({
      scopes: ["project"],
      statuses: ["approved"],
      kinds: ["adr"],
      includeSources: false,
      limit: 500,
    });
  });

  it("registers the knowledge section after memory on desktop and web", () => {
    for (const options of [{ isDesktop: true }, {}]) {
      const { settingsSectionGroups } = createSettingsPageConfig(options);
      const agent = settingsSectionGroups.find((group) => group.id === "agentCapabilities");
      const sectionIds = agent?.sections.map((section) => section.id) ?? [];
      expect(sectionIds).toContain("knowledge");
      expect(sectionIds.indexOf("knowledge")).toBe(sectionIds.indexOf("memory") + 1);
    }
  });
});

function render(locale: "en-US" | "zh-CN", element: ReturnType<typeof createElement>) {
  return renderToStaticMarkup(
    createElement(ESCodeIntlProvider, { initialLocale: locale }, element),
  );
}

const entryNode: V4WikiGraphNode = {
  id: "project:api-errors",
  type: "entry",
  label: "API error envelope",
  scope: "project",
  entryId: "api-errors",
  kind: "contract",
  status: "proposal",
};

const entryDetail: V4WikiEntryResult = {
  found: true,
  entry: {
    id: "api-errors",
    title: "API error envelope",
    scope: "project",
    kind: "contract",
    status: "proposal",
    authority: "proposal",
    validity: "stale",
    validity_reason: "source changed",
    origin: "imported",
    tags: ["api"],
    related: [],
    updated_at: "2026-10-01T00:00:00Z",
    body: "Errors use { code, message }.",
    sources: [
      {
        document_id: "doc-1",
        source_path: "docs/api.docx",
        format: "docx",
        locator: "heading:Errors",
        excerpt: "All errors…",
        content_hash: "abc",
        source_status: "changed",
      },
    ],
  },
  conflicts: [
    {
      kind: "contradiction",
      parties: [
        {
          id: "api-errors",
          scope: "project",
          status: "proposal",
          authority: "proposal",
          updated_at: "",
          sources: [],
        },
        {
          id: "error-format",
          scope: "global",
          status: "approved",
          authority: "approved-global-standard",
          updated_at: "",
          sources: [],
        },
      ],
      evidence: "different envelope fields",
      suggestion: "deprecate one",
    },
  ],
  review: [{ action: "proposed", at: "2026-10-01", by: "cli" }],
};

describe("knowledge settings rendering", () => {
  it("shows the unsupported runtime notice instead of the graph", () => {
    const html = render(
      "en-US",
      createElement(KnowledgePhaseNotice, {
        phase: "unsupported",
        error: null,
        status: { supported: false, enabled: false } satisfies V4WikiStatusResult,
        t: (id: string) => id,
        onRetry: () => undefined,
      }),
    );
    expect(html).toContain('data-phase="unsupported"');
    expect(html).toContain("settings.knowledge.unsupportedHint");
  });

  it("renders nothing for the ready phase", () => {
    const html = render(
      "en-US",
      createElement(KnowledgePhaseNotice, {
        phase: "ready",
        error: null,
        status: null,
        t: (id: string) => id,
        onRetry: () => undefined,
      }),
    );
    expect(html).toBe("");
  });

  it("renders entry body, provenance, conflicts, history and proposal actions", () => {
    const html = render(
      "zh-CN",
      createElement(KnowledgeEntryDetail, {
        node: entryNode,
        detail: entryDetail,
        loading: false,
        busy: false,
        canManage: true,
        onClose: () => undefined,
        onReview: () => undefined,
      }),
    );
    expect(html).toContain("Errors use { code, message }.");
    expect(html).toContain("docs/api.docx");
    expect(html).toContain("heading:Errors");
    expect(html).toContain("error-format");
    expect(html).toContain("source changed");
    expect(html).toContain('data-testid="knowledge-entry-approve"');
    expect(html).toContain('data-testid="knowledge-entry-reject"');
    expect(html).not.toContain('data-testid="knowledge-entry-deprecate"');
    expect(html).toContain("批准");
  });

  it("hides governance actions when the service cannot manage the wiki", () => {
    const html = render(
      "en-US",
      createElement(KnowledgeEntryDetail, {
        node: entryNode,
        detail: entryDetail,
        loading: false,
        busy: false,
        canManage: false,
        onClose: () => undefined,
        onReview: () => undefined,
      }),
    );
    expect(html).not.toContain("knowledge-entry-actions");
  });
});
