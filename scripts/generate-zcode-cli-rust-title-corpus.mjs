// Run with node --import tsx. 会话标题纯规则的 TS oracle（docs/specs/rust-session-title.md）：
// system prompt、first_input 标题公式、输入归一与生成标题清洗/解析。--check 防漂移。
// 语料只含确定性字符串，不含平台路径与时间，三平台输出一致。
import { readFile, writeFile } from "node:fs/promises";
import { titleFromInput } from "../apps/zcode-cli/packages/core/src/runtime/helpers/project.ts";
import {
  MAX_TITLE_CHARS,
  MAX_TITLE_INPUT_CHARS,
  SESSION_TITLE_SYSTEM_PROMPT,
  TITLE_GENERATION_TIMEOUT_MS,
  cleanGeneratedTitle,
  normalizeTitleInput,
} from "../apps/zcode-cli/packages/core/src/runtime/methods/title-generation-sidecar.ts";
import { fallbackGoalSummaryTitle } from "../apps/zcode-cli/packages/core/src/runtime/methods/goal-summary-title.ts";

// TS `session-title.ts` 的短输入门槛：按 code point 计。
const MIN_GENERATED_TITLE_INPUT_CHARS = 10;

const firstInput = [
  "",
  "   ",
  "\n\t ",
  "hi",
  "  hello   world  ",
  "line one\nline two",
  "tab\there",
  "a".repeat(59),
  "a".repeat(60),
  "a".repeat(61),
  "a".repeat(200),
  "修复  登录  页面",
  "😀 emoji title",
  "a".repeat(56) + "😀",
  "a".repeat(58) + "😀",
  "  多行\n输入\t折叠   ",
].map((input) => ({ input, title: titleFromInput(input) }));

const normalized = [
  "",
  "   ",
  " hi ",
  "a  b\n\tc",
  "a".repeat(1199),
  "a".repeat(1200),
  "a".repeat(1201),
  "😀".repeat(700),
  "  中间  空白\n换行 ",
].map((input) => ({ input, normalized: normalizeTitleInput(input) }));

const shortGuard = [
  "",
  "  ",
  "short",
  "9 chars!!",
  "1234567890",
  "0123456789",
  "😀".repeat(5),
  "😀".repeat(10),
  "修复登录页面", // 6 code points
  "修复登录页面现在开始",
].map((input) => {
  const value = normalizeTitleInput(input);
  return {
    input,
    normalized: value,
    codePoints: Array.from(value).length,
    passes: Array.from(value).length >= MIN_GENERATED_TITLE_INPUT_CHARS,
  };
});

const cleaned = [
  '{"title":"Fix login bug"}',
  '{"title":"  spaced  "}',
  '{"title":""}',
  '{"title":42}',
  "[]",
  "not json at all",
  "first line wins\nsecond line",
  "\n\n  leading blank lines  \nrest",
  "```json\n{\"title\":\"Fenced JSON\"}\n```",
  "```\n{\"title\":\"Fence without tag\"}\n```",
  "```json\nnot json inside fence\n```",
  '{"title":"with trailing punct!!!"}',
  '{"title":"## Heading style"}',
  '{"title":"\\"quoted\\""}',
  '{"title":"“curly quotes”"}',
  '{"title":"no letters here ..."}',
  '{"title":"end with colon:"}',
  '{"title":"Hello, world, and more;"}',
  '{"title":"emoji 😀 only?"}',
  '{"title":"😀😀😀"}',
  '{"title":"' + "word ".repeat(30).trim() + '"}',
  // 尖括号以 \u003c/\u003e 转义书写：直接写字面量会被工具链改写（见 docs/specs/rust-session-title.md 备注）。
  "thinking prefix\n\u003cthink\u003einternal reasoning\u003c/think\u003e{\"title\":\"After think\"}",
  "\u003cthink\u003e\u53ea\u6709\u601d\u8003\u003c/think\u003e",
  "\u003cTHINK\u003eUpper case tag\u003c/THINK\u003e\u003cthink\u003esecond block\u003c/think\u003eActual",
  '{"other":1}',
  "{ broken json",
  "```JSON\n{\"title\":\"Upper fence tag\"}\n```",
  '{"title":"trailing whitespace   "}',
  '{"title":"###"}',
  '{"title":"..."}',
  '{"title":"a"}',
].map((raw) => ({ raw, title: cleanGeneratedTitle(raw) }));

const goalFallback = [
  "",
  "   ",
  "fix it",
  "  重构   解析器  ",
  "a".repeat(100),
  "a".repeat(101),
  "word ".repeat(40),
  // 截断点落在代理对中间时 JS 会留下孤立代理项，Rust String 无法表示（见 spec「已知差异」）；
  // 语料只取截断点在代理对边界上的情形。
  "a" + "😀".repeat(60),
].map((objective) => ({ objective, title: fallbackGoalSummaryTitle(objective) }));

const content = `${JSON.stringify(
  {
    systemPrompt: SESSION_TITLE_SYSTEM_PROMPT,
    constants: {
      maxTitleInputChars: MAX_TITLE_INPUT_CHARS,
      maxTitleChars: MAX_TITLE_CHARS,
      timeoutMs: TITLE_GENERATION_TIMEOUT_MS,
      minGeneratedTitleInputChars: MIN_GENERATED_TITLE_INPUT_CHARS,
    },
    firstInput,
    normalized,
    shortGuard,
    cleaned,
    goalFallback,
  },
  null,
  1,
)}\n`;
const write = async (path, value) => {
  if (process.argv.includes("--check")) {
    const current = await readFile(path, "utf8").catch(() => "");
    if (current !== value) throw new Error(`Rust title asset differs from TS: ${path}`);
  } else await writeFile(path, value);
};
await write(
  new URL("../apps/zcode-cli-rust/crates/domain/tests/fixtures/title_corpus.json", import.meta.url),
  content,
);
// system prompt 作为生成资产直接进二进制（include_str!），避免两边各写一份。
await write(
  new URL("../apps/zcode-cli-rust/crates/domain/src/session_title_prompt.txt", import.meta.url),
  SESSION_TITLE_SYSTEM_PROMPT,
);
