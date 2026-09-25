// Run with node --import tsx. 以 TS WebFetch 的纯逻辑为 oracle 导出语料：URL 规范化、字面量出网拦截、
// 重定向判定、正文抽取（HTML→Markdown）、截断与提示词处理。Rust domain::web_fetch 逐条比对（--check 防漂移）。
// 见 docs/specs/rust-webfetch.md。
import { readFile, writeFile } from "node:fs/promises";
import {
  isPermittedRedirect,
  normalizeWebFetchUrl,
  resolveRedirectUrl,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/webfetch-url.ts";
import { assertWebFetchLiteralEgress } from "../apps/zcode-cli/packages/core/src/tool/handlers/webfetch-egress-guard.ts";
import {
  extractReadableContent,
  truncateContentForModel,
} from "../apps/zcode-cli/packages/core/src/tool/handlers/webfetch-content.ts";
import { processFetchedContent } from "../apps/zcode-cli/packages/core/src/tool/handlers/webfetch-processing.ts";

const attempt = (fn) => {
  try {
    return { ok: fn() };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
};

const urls = [
  "https://example.com/a?b=1#c",
  "http://example.com",
  "http://example.com:80/x",
  "http://example.com:443/",
  "HTTPS://Example.COM/Path",
  "ftp://example.com/",
  "https://user:pw@example.com/",
  "https://user@example.com/",
  "https://localhost/",
  "https://a.localhost/",
  "https://printer.local/",
  "https://intranet/",
  "https://example.com./",
  "  https://example.com/trim  ",
  "https://例え.jp/パス?q=値",
  "https://www.example.com/",
  "https://example.com:8443/",
  "not a url",
  `https://example.com/${"a".repeat(2100)}`,
  "https://127.0.0.1/",
  "https://127.1/",
  "https://2130706433/",
  "https://0x7f.0.0.1/",
  "https://10.0.0.1/",
  "https://172.16.5.4/",
  "https://192.168.1.1/",
  "https://8.8.8.8/",
  "https://100.64.1.1/",
  "https://169.254.169.254/latest",
  "https://192.0.0.1/",
  "https://192.0.2.5/",
  "https://192.88.99.1/",
  "https://198.18.0.1/",
  "https://198.19.255.255/",
  "https://198.51.100.1/",
  "https://203.0.113.9/",
  "https://240.0.0.1/",
  "https://0.0.0.0/",
  "https://255.255.255.255/",
  "https://224.0.0.1/",
  "https://[::1]/",
  "https://[::]/",
  "https://[2606:4700::1111]/",
  "https://[::ffff:127.0.0.1]/",
  "https://[::ffff:8.8.8.8]/",
  "https://[64:ff9b::a00:1]/",
  "https://[64:ff9b::808:808]/",
  "https://[64:ff9b:1::1]/",
  "https://[100::1]/",
  "https://[2001:2::1]/",
  "https://[2001:10::1]/",
  "https://[2001:20::1]/",
  "https://[2001:db8::1]/",
  "https://[2001::1]/",
  "https://[2002::1]/",
  "https://[fe80::1]/",
  "https://[fc00::1]/",
  "https://[ff02::1]/",
  "https://[::ffff:0:1.2.3.4]/",
];
const urlCases = urls.map((input) => {
  const normalized = attempt(() => normalizeWebFetchUrl(input).toString());
  const egress =
    normalized.ok === undefined
      ? null
      : (attempt(() => assertWebFetchLiteralEgress(new URL(normalized.ok))).error ?? null);
  return { input, normalized, egress };
});

const redirectBases = [
  "https://example.com/a/b",
  "https://www.example.com:8443/",
  "https://8.8.8.8/x",
];
const locations = [
  "/next",
  "c",
  "https://www.example.com/x",
  "https://WWW.Example.com/x",
  "https://other.com/",
  "http://example.com/x",
  "https://example.com:444/",
  "https://example.com:443/",
  "https://user:p@example.com/",
  "https://127.0.0.1/",
  "//example.com/y",
  "https://www.www.example.com/",
  "https://example.com:8443/",
  "https://8.8.8.8/y",
  "http://[bad",
];
const redirectCases = [];
for (const base of redirectBases)
  for (const location of locations) {
    const resolved = attempt(() => resolveRedirectUrl(location, new URL(base)).toString());
    const permitted =
      resolved.ok === undefined ? null : isPermittedRedirect(new URL(base), new URL(resolved.ok));
    redirectCases.push({ base, location, resolved, permitted });
  }

const html = [
  "<html><head><title>T</title><style>p{}</style><script>var a = '<p>';</script></head><body><h1>Title</h1><p>Hello <b>world</b>!</p></body></html>",
  "<!-- note --><H2 class='x'>Upper</H2><noscript>no</noscript><div>one</div><div>two</div>",
  '<ul><li>a</li><li><a href="https://e.com/x">link</a></li></ul><a href=\'/rel\'>single</a><a name="n">none</a>',
  "line1<br>line2<br/>line3<BR />   spaced    out\t\ttabs",
  "&amp;lt; &nbsp;x&nbsp; &quot;q&quot; &#39;s&#39; &#x4e2d;&#20013; &#x1F600; &gt;",
  "<p>a</p>\n\n\n\n<p>b</p>\n   \n<table><tr><td>c</td></tr></table>",
  "   \n\n  <p>  lead  </p>  \n\n",
  "<h3>h3</h3><h4>h4</h4><h5>h5</h5><h6>h6</h6><section>s</section><article>a</article><header>h</header><footer>f</footer><ol><li>o</li></ol>",
  "<a href=\"x\"\n>multi\nline</a><script\ntype='x'>hidden</script>",
];
const contentCases = [];
for (const body of html) contentCases.push({ contentType: "text/html; charset=utf-8", body });
for (const [contentType, body] of [
  ["application/json", '  {"a": 1}  \n'],
  ["text/plain", "\n plain text \n"],
  ["", "  no type  "],
  ["application/xhtml+xml", "<p>xhtml</p>"],
  ["application/vnd.api+json", '{"x":1}'],
  ["application/rss+xml", "<rss/>"],
  ["text/x-html-fragment", "<b>frag</b>"],
  ["image/png", "binary"],
  ["application/pdf", "%PDF"],
  ["TEXT/HTML", "<P>caps</P>"],
  ["text/markdown", "# md\n\ntext"],
]) {
  contentCases.push({ contentType, body });
}
contentCases.push({ contentType: "text/plain", body: "﻿bom text" });
contentCases.push({
  contentType: "text/plain",
  bodyBase64: Buffer.from([0x61, 0xff, 0xfe, 0x62, 0xe4, 0xb8]).toString("base64"),
});
contentCases.push({ contentType: "text/html", body: "&#x110000; out of range" });
for (const item of contentCases) {
  const bytes = item.bodyBase64
    ? Buffer.from(item.bodyBase64, "base64")
    : Buffer.from(item.body, "utf8");
  item.result = attempt(() => extractReadableContent(new Uint8Array(bytes), item.contentType));
}

// 长输入以 fill × count + tail 描述，输出以长度（UTF-16）与首尾片段比对，控制语料体积。
const truncateInputs = [
  { fill: "s", count: 0, tail: "short" },
  { fill: "x", count: 100_000, tail: "" },
  { fill: "x", count: 100_001, tail: "" },
  { fill: "x", count: 99_945, tail: `😀${"t".repeat(100)}` },
  { fill: "y", count: 99_944, tail: `😀${"t".repeat(100)}` },
  { fill: "字", count: 100_050, tail: "" },
];
const truncateCases = truncateInputs.map((input) => {
  const { content, truncated } = truncateContentForModel(
    input.fill.repeat(input.count) + input.tail,
  );
  // 截断可能切开代理对；Rust 字符串不能表示孤立代理项，统一按 U+FFFD 比对（见 rust-webfetch.md）。
  const wellFormed = content.toWellFormed();
  return {
    input,
    length: wellFormed.length,
    head: wellFormed.slice(0, 20),
    tail: wellFormed.slice(-120),
    truncated,
  };
});

const processingCases = [];
for (const preapprovedUrl of [false, true])
  for (const contentType of ["text/markdown; charset=utf-8", "text/html"])
    for (const content of ["# Doc\n\nbody", "z".repeat(100_001)])
      for (const modelText of ["  answer  ", "   "]) {
        let captured;
        const model = {
          optionSpecs: {
            reasoningLevel: { values: ["low", "high"] },
            maxOutputTokens: { max: 3000 },
          },
          async generateText(request) {
            captured = request;
            return { text: modelText };
          },
        };
        const result = await processFetchedContent(
          { url: "https://docs.python.org/x", prompt: "What is it?" },
          {
            content,
            contentType,
            finalUrl: "https://docs.python.org/x",
            bytes: 1,
            redirects: [],
            sizeBytes: 1,
            status: 200,
            statusText: "OK",
          },
          { model, toolCallId: "t", traceId: "trace" },
          { preapprovedUrl },
        );
        processingCases.push({
          preapprovedUrl,
          contentType,
          content:
            content.length > 200 ? { repeat: content.slice(0, 1), count: content.length } : content,
          modelText,
          prompt: captured?.messages?.[0]?.content?.toWellFormed() ?? null,
          maxOutputTokens: captured?.options?.maxOutputTokens ?? null,
          result,
        });
      }

// 长字符串用摘要表示，避免语料体积过大。
for (const c of processingCases)
  if (c.prompt && c.prompt.length > 400)
    c.prompt = {
      head: c.prompt.slice(0, 200),
      tail: c.prompt.slice(-400),
      length: c.prompt.length,
    };
for (const c of urlCases)
  if (c.input.length > 300) c.input = { prefix: c.input.slice(0, 20), repeat: "a", count: 2100 };

const content = `${JSON.stringify({ urlCases, redirectCases, contentCases, truncateCases, processingCases })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/webfetch_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content)
    throw new Error("Rust WebFetch corpus differs from TS");
} else await writeFile(target, content);
