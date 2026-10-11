import { describe, expect, it } from "vitest";
import { projectCircuitReferences } from "@/circuit/domain/circuitReferences.js";

/**
 * `::circuit` 引用解析（docs/circuit/README.md §8、§10.2）。
 * 规则与 Gen UI 引用一致：代码块内不识别、消息完成后才渲染、流式半截先隐藏；
 * 载荷必须过 `circuitReferenceSchema`（元件数 / 网络数是数字，key=value 形态给不了）。
 */

const jsonDirective = '::circuit{"irId":"ir:0123456789abcdef","title":"RP2040_minimal"}';
const withCounts =
  '::circuit{"irId":"ir:0123456789abcdef","title":"RP2040_minimal","componentCount":24,"netCount":41}';
const inlineDirective = '::circuit{irId="ir:0123456789abcdef" title="RP2040_minimal"}';

describe("::circuit 引用投影", () => {
  it("在完成的消息里渲染 JSON 形态的引用并保留偏移", () => {
    expect(projectCircuitReferences(`前面\n${jsonDirective}\n后面`, true)).toEqual([
      { kind: "text", text: "前面\n" },
      {
        kind: "circuit",
        reference: { irId: "ir:0123456789abcdef", title: "RP2040_minimal" },
        offset: 3,
      },
      { kind: "text", text: "\n后面" },
    ]);
  });

  it("接受 key=value 形态的 irId 与 title", () => {
    expect(projectCircuitReferences(inlineDirective, true)).toEqual([
      {
        kind: "circuit",
        reference: { irId: "ir:0123456789abcdef", title: "RP2040_minimal" },
        offset: 0,
      },
    ]);
  });

  it("保留降级卡片要用的器件数与网络数", () => {
    expect(projectCircuitReferences(withCounts, true)[0]).toMatchObject({
      kind: "circuit",
      reference: { componentCount: 24, netCount: 41 },
    });
  });

  it("同一段正文里的多处引用都各自渲染", () => {
    const text = `${jsonDirective}\n\n${withCounts}`;
    const parts = projectCircuitReferences(text, true).filter((part) => part.kind === "circuit");
    expect(parts).toHaveLength(2);
    expect(parts.map((part) => part.kind === "circuit" && part.offset)).toEqual([
      0,
      jsonDirective.length + 2,
    ]);
  });

  it.each([
    `\`${jsonDirective}\``,
    `\`\`\`text\n${jsonDirective}\n\`\`\``,
    `~~~text\n${jsonDirective}\n~~~`,
    `<pre>${jsonDirective}</pre>`,
    `\`${inlineDirective}\``,
  ])("代码块里的示例不生效：%s", (text) => {
    for (const completed of [true, false]) {
      expect(projectCircuitReferences(text, completed)).toEqual([{ kind: "text", text }]);
    }
  });

  it("未完成的消息只挂起，不提前渲染卡片", () => {
    for (let length = 2; length <= jsonDirective.length; length += 1) {
      const parts = projectCircuitReferences(`前面 ${jsonDirective.slice(0, length)}`, false);
      expect(parts.some((part) => part.kind === "circuit")).toBe(false);
      expect(parts.filter((part) => part.kind === "text")).toEqual([
        { kind: "text", text: "前面 " },
      ]);
    }
    expect(projectCircuitReferences(jsonDirective, false)).toEqual([{ kind: "pending" }]);
  });

  it("完成后仍缺右括号的载荷算无效，不吞掉后文", () => {
    expect(projectCircuitReferences('::circuit{"irId":"ir:1"', true)).toEqual([
      { kind: "invalid" },
    ]);
    expect(
      projectCircuitReferences('::circuit{"irId":"ir:1"\n这里是普通正文。', false),
    ).toEqual([{ kind: "text", text: '::circuit{"irId":"ir:1"\n这里是普通正文。' }]);
  });

  it.each([
    '::circuit{"title":"没有 irId"}',
    '::circuit{"irId":"ir:1","componentCount":"24"}',
    '::circuit{"irId":"ir:1","netCount":-1}',
    '::circuit{"irId":""}',
    '::circuit{irId="ir:1" componentCount="24"}',
    '::circuit{"irId":"ir:1","unknown":true}',
    '::circuit{"irId":"ir:1"',
  ])("拒绝不合法载荷：%s", (text) => {
    expect(projectCircuitReferences(text, true)).toEqual([{ kind: "invalid" }]);
  });

  it.each([
    ':circuit{"irId":"ir:1"}',
    ':::circuit{"irId":"ir:1"}',
    '::circuitry{"irId":"ir:1"}',
  ])("不扩展到其它 directive 语法：%s", (text) => {
    expect(projectCircuitReferences(text, true)).toEqual([{ kind: "text", text }]);
  });
});

describe("电路引用 chip 的载荷", () => {
  it("器件用位号，网络用 NET: 前缀，都带 irId", async () => {
    const { circuitComponentDraftReference, circuitNetDraftReference } = await import(
      "@/circuit/domain/circuitReferences.js"
    );
    expect(
      circuitComponentDraftReference("ir:1", { id: "uuid-u3", ref: "U3" }),
    ).toMatchObject({ irId: "ir:1", kind: "component", label: "U3" });
    expect(
      circuitNetDraftReference("ir:1", {
        id: "net:I2C1_SDA",
        name: "I2C1_SDA",
        pinRefs: [],
        origin: { by: "parser", confidence: 1 },
      }),
    ).toMatchObject({ irId: "ir:1", kind: "net", label: "NET:I2C1_SDA", name: "I2C1_SDA" });
  });

  it("网络没有名字时退化成网络 id，不把空格带进 chip", async () => {
    const { circuitNetDraftReference } = await import("@/circuit/domain/circuitReferences.js");
    expect(
      circuitNetDraftReference("ir:1", {
        id: "net:Net-(R1-Pad2)",
        pinRefs: [],
        origin: { by: "parser", confidence: 1 },
      }),
    ).toMatchObject({ label: "NET:net:Net-(R1-Pad2)", name: "net:Net-(R1-Pad2)" });
  });
});
