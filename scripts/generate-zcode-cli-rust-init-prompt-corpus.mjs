// Run with node --import tsx. 以 TS resolveZCodeBuiltinPromptCommand 为 oracle，导出 `/init` 展开语料。
// 路径分隔符随平台（Windows `\`、POSIX `/`），Rust 端比较时统一归一，产物因此与生成机平台无关。
import { readFile, writeFile } from "node:fs/promises";
import { resolveZCodeBuiltinPromptCommand } from "../apps/zcode-cli/packages/bootstrap/src/builtin-prompt-command.ts";

const workingDirectory = "/workspace/project";
const inputs = [
  "/init",
  "/init ",
  "/init add CI notes",
  "/init   multi   space  ",
  "/INIT uppercase",
  "/Init Mixed",
  "/init\nmultiline\nargs",
  "/init `code` and $VAR",
  "/init 中文说明",
  " /init padded ",
  "hello /init",
  "/compact",
  "/workflow",
  "/goal x",
  "/unknown arg",
  "/initx",
  "/init-extra",
  "",
  "   ",
  "/",
];

const cases = inputs.map((input) => {
  const prompt = resolveZcode();
  function resolveZcode() {
    // Rust 无动态工作流，恒按关闭处理：`/workflow` 因此不展开（TS 在开关为 false 时一致）。
    return (
      resolveZCodeBuiltinPromptCommand(input, {
        workingDirectory,
        dynamicWorkflowEnabled: false,
      }) ?? null
    );
  }
  // TS 用 path.join 拼路径，分隔符随生成机平台；统一成 `/`，否则 POSIX CI 上 --check 必然漂移（Rust 端比较前同样归一）。
  return [input, prompt === null ? null : prompt.replaceAll("\\", "/")];
});
if (!cases.some(([, prompt]) => prompt)) throw new Error("corpus has no expanded case");

const content = `${JSON.stringify({ workingDirectory, cases })}\n`;
const target = new URL(
  "../apps/zcode-cli-rust/crates/domain/tests/fixtures/init_prompt_corpus.json",
  import.meta.url,
);
if (process.argv.includes("--check")) {
  if ((await readFile(target, "utf8")) !== content) {
    throw new Error(
      "Init prompt corpus differs from TS; run node --import tsx scripts/generate-zcode-cli-rust-init-prompt-corpus.mjs",
    );
  }
} else {
  await writeFile(target, content);
}
