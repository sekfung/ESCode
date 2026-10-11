#!/usr/bin/env node
/**
 * 电路识别离线评测（docs/circuit/README.md §10.4；**不作为 CI 门禁**）。
 *
 * 用法：
 *   node scripts/circuit-recognition-eval.mjs --model <标签> --recognitions <目录> [选项]
 *
 * 它做三件事：
 * 1. **语料**：用本机 KiCad 官方 CLI（kicad-cli）从 3 块 fixture 板子导出矢量 PDF 与截图；
 *    没有 kicad-cli 时报错并给出安装 / 手工放置素材的说明（不静默跳过、不用别的渲染器冒充 KiCad 输出）。
 * 2. **标准答案**：用 CircuitService 解析同一块板子的 `.kicad_sch`，取它的 IR。
 * 3. **比对与报告**：读入外部给出的识别草稿 JSON（模型/Agent 会话产出），逐元素比对，
 *    统计器件与网络的准确率/召回率，输出 Markdown 报告（模型标签、样本数、每块板与合计、
 *    未匹配元素清单）；比对口径见 scripts/lib/circuit-recognition-metrics.mjs。
 *
 * 语料与报告默认写在仓库的 `.tmp/circuit-recognition-eval/`（已 gitignore），仓库里不放生成的 PDF/PNG。
 */
import { execFile } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import { tsImport } from "tsx/esm/api";
import {
  aggregateResults,
  comparisonView,
  compareRecognition,
  formatRatio,
  standardView,
} from "./lib/circuit-recognition-metrics.mjs";

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(import.meta.dirname, "..");
const defaultCorpusDir = join(repositoryRoot, ".tmp", "circuit-recognition-eval");
const FIXTURE_ROOT = resolve(repositoryRoot, "packages/services/test/fixtures/circuit");

/** 3 块 fixture 板子（§10.1）：标准答案来自这里的 `.kicad_sch`，语料由它们导出。 */
const BOARDS = [
  { id: "rp2040-minimal", schematic: "rp2040-minimal/RP2040_minimal.kicad_sch" },
  { id: "usbc-buck-3v3", schematic: "usbc-buck-3v3/USB power port.kicad_sch" },
  { id: "mpu6050-imu", schematic: "mpu6050-imu/MPU6050.kicad_sch" },
];

function parseArgs(argv) {
  const options = {
    corpusDir: defaultCorpusDir,
    recognitions: undefined,
    model: undefined,
    skipCorpus: false,
    boards: [],
  };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--skip-corpus") options.skipCorpus = true;
    else if (flag === "--corpus") options.corpusDir = resolve(argv[++index]);
    else if (flag === "--recognitions") options.recognitions = resolve(argv[++index]);
    else if (flag === "--model") options.model = argv[++index];
    else if (flag === "--board") options.boards.push(argv[++index]);
    else if (flag === "--help" || flag === "-h") options.help = true;
    else throw new Error(`未知参数：${flag}`);
  }
  return options;
}

const HELP = `用法: node scripts/circuit-recognition-eval.mjs --model <标签> --recognitions <目录> [选项]

  --model <标签>         识别所用的模型标签（写进报告；必填）
  --recognitions <目录>  识别草稿 JSON 目录，文件名 <board>.<pdf|png>.json（必填；--skip-corpus 时也要）
  --corpus <目录>        语料目录（默认 .tmp/circuit-recognition-eval）
  --skip-corpus          不跑 kicad-cli，直接用已有语料与标准答案
  --board <id>           只评测某块板（可重复；默认全部 3 块）
  --help                 显示本说明

素材放置约定（不想装 KiCad CLI 时）：
  <corpus>/<board>.pdf 与 <corpus>/<board>.png 手工放进去即可，识别草稿按 <board>.<pdf|png>.json 放。`;

/** kicad-cli 探测：返回版本字符串；缺失时返回 undefined（调用方报错并给安装说明）。 */
async function probeKiCadCli() {
  try {
    const { stdout } = await execFileAsync("kicad-cli", ["version"], { timeout: 30_000 });
    return stdout.trim() || "unknown";
  } catch {
    return undefined;
  }
}

async function kicadCli(args) {
  const { stdout, stderr } = await execFileAsync("kicad-cli", args, {
    maxBuffer: 32 * 1024 * 1024,
  });
  return `${stdout}${stderr}`.trim();
}

const KICAD_INSTALL_HINT = [
  "本机没有可执行的 kicad-cli，无法生成评测语料。",
  "安装方式（KiCad 官方发行版自带 CLI）：",
  "  - Windows: winget install KiCad.KiCad  （或从 https://www.kicad.org/download/ 安装，kicad-cli 在安装目录的 bin/ 下）",
  "  - macOS:   brew install --cask kicad",
  "  - Linux:   apt install kicad  （或发行版对应的包）",
  `也可以手工准备素材（输出目录：${defaultCorpusDir}）：`,
  "  mkdir -p .tmp/circuit-recognition-eval",
  "  # 把每块板的矢量 PDF 放成 <board>.pdf、截图放成 <board>.png（board 见下），再带 --skip-corpus 运行。",
  `板子 id：${BOARDS.map((board) => board.id).join(", ")}`,
  "识别草稿 JSON 按 <board>.<pdf|png>.json 放在 --recognitions 目录里（草稿 schema 见 shared 的 circuitRecognitionDraftSchema）。",
].join("\n");

/**
 * 用 kicad-cli 导出语料（§10.4）：PDF 用 `sch export pdf`；PNG 先用 `sch export png`，
 * 该版本不支持 png 时导出官方 SVG 并要求用户自行栅格化——不拿别的渲染器冒充 KiCad 输出。
 */
async function generateCorpus(corpusDir, boardIds) {
  const version = await probeKiCadCli();
  if (!version) throw new Error(KICAD_INSTALL_HINT);
  await mkdir(corpusDir, { recursive: true });
  const commands = [];
  const exported = await kicadCli(["sch", "export", "--help"]).catch(() => "");
  const supportsPng = /\bpng\b/u.test(exported);
  for (const board of BOARDS.filter(
    (candidate) => boardIds.length === 0 || boardIds.includes(candidate.id),
  )) {
    const schematic = join(FIXTURE_ROOT, board.schematic);
    const pdfPath = join(corpusDir, `${board.id}.pdf`);
    const pngPath = join(corpusDir, `${board.id}.png`);
    await kicadCli(["sch", "export", "pdf", "--output", pdfPath, schematic]);
    commands.push(`kicad-cli sch export pdf --output ${pdfPath} "${schematic}"`);
    if (supportsPng) {
      await kicadCli(["sch", "export", "png", "--output", pngPath, schematic]);
      commands.push(`kicad-cli sch export png --output ${pngPath} "${schematic}"`);
    } else {
      const svgPath = join(corpusDir, `${board.id}.svg`);
      await kicadCli(["sch", "export", "svg", "--output", svgPath, schematic]);
      commands.push(`kicad-cli sch export svg --output ${svgPath} "${schematic}"`);
      if (!existsSync(pngPath))
        throw new Error(
          [
            `kicad-cli ${version} 的 \`sch export\` 没有 png 子命令（只有 svg / pdf / dxf / ps / …），截图需要手工准备：`,
            `  1. 用 KiCad GUI 打开 ${schematic}，截图存成 ${pngPath}；或`,
            `  2. 把刚导出的 ${svgPath} 用任意 SVG 栅格化工具转成 PNG 存成 ${pngPath}。`,
            "  3. 放好后带 --skip-corpus 重新运行本脚本。",
            "（本脚本不会用别的渲染器生成截图冒充 KiCad 导出。）",
          ].join("\n"),
        );
    }
  }
  return { version, commands };
}

/** 标准答案：用 CircuitService（真实服务 + 真实 workspace 读取适配器）解析 `.kicad_sch`。 */
async function loadStandardAnswers(boardIds) {
  const { createCircuitService } = await tsImport(
    pathToFileURL(join(repositoryRoot, "packages/services/src/circuit/app/circuitService.ts")).href,
    import.meta.url,
  );
  const { createWorkspaceSource } = await tsImport(
    pathToFileURL(join(repositoryRoot, "packages/services/src/circuit/adapters/workspaceFiles.ts"))
      .href,
    import.meta.url,
  );
  const records = new Map();
  const service = createCircuitService({
    ports: {
      store: {
        async read({ identityKey, sourcePath }) {
          return records.get(`${identityKey}\u0000${sourcePath}`);
        },
        async findByIrId() {
          return undefined;
        },
        async write({ identityKey, sourcePath, record }) {
          records.set(`${identityKey}\u0000${sourcePath}`, record);
        },
      },
      source: createWorkspaceSource(),
      watcher: {
        async watch() {
          return { dispose: () => undefined };
        },
      },
      snapshot: {
        async write() {
          throw new Error("io_error: 评测不写快照");
        },
      },
      vector: {
        async extract() {
          throw new Error("io_error: 评测标准答案只用确定性解析，不需要 PDF 提取");
        },
      },
    },
  });
  const answers = new Map();
  for (const board of BOARDS.filter(
    (candidate) => boardIds.length === 0 || boardIds.includes(candidate.id),
  )) {
    // CircuitService 的 info 日志走 console；评测的标准输出只需要报告，解析期间临时静音。
    const view = await withMutedConsole(() =>
      service.parse({ workspacePath: FIXTURE_ROOT, path: board.schematic }),
    );
    const ir = await readStoredIr(records, FIXTURE_ROOT, board.schematic);
    answers.set(board.id, { summary: view.summary, view: standardView(ir) });
  }
  service.dispose();
  return answers;
}

async function withMutedConsole(task) {
  const original = console.log;
  console.log = () => undefined;
  try {
    return await task();
  } finally {
    console.log = original;
  }
}

async function readStoredIr(records, workspacePath, sourcePath) {
  const record = records.get(`${workspacePath}\u0000${sourcePath}`);
  if (!record) throw new Error(`not_found: 标准答案未生成（${sourcePath}）`);
  return record.ir;
}

/** 识别草稿：<recognitions>/<board>.<pdf|png>.json（严格用 shared 的草稿 schema 校验）。 */
async function loadRecognitions(recognitionsDir, boardIds) {
  const { circuitRecognitionDraftSchema } = await tsImport(
    pathToFileURL(join(repositoryRoot, "packages/shared/src/circuit/ir.ts")).href,
    import.meta.url,
  );
  const entries = await readdir(recognitionsDir).catch(() => {
    throw new Error(`not_found: 识别草稿目录不存在：${recognitionsDir}`);
  });
  const drafts = new Map();
  for (const entry of entries) {
    const match = /^(?<board>.+)\.(?<source>pdf|png)\.json$/u.exec(entry);
    if (!match) continue;
    const boardId = match.groups.board;
    if (boardIds.length > 0 && !boardIds.includes(boardId)) continue;
    const parsed = circuitRecognitionDraftSchema.safeParse(
      JSON.parse(await readFile(join(recognitionsDir, entry), "utf-8")),
    );
    if (!parsed.success)
      throw new Error(`invalid_ir: ${entry} 不是合法识别草稿：${parsed.error.message}`);
    drafts.set(`${boardId}.${match.groups.source}`, {
      boardId,
      source: match.groups.source,
      view: comparisonView(parsed.data),
    });
  }
  if (drafts.size === 0)
    throw new Error(
      `not_found: ${recognitionsDir} 里没有 <board>.<pdf|png>.json 形态的识别草稿（板子：${BOARDS.map((board) => board.id).join(", ")}）`,
    );
  return drafts;
}

function reportLines({ model, corpusDir, kicad, answers, results }) {
  const lines = [
    "# 电路识别离线评测（§10.4）",
    "",
    `- 模型标签：${model}`,
    `- 样本数：${results.length}（板子 × 来源）／语料目录：\`${corpusDir}\``,
    `- kicad-cli：${kicad.version}`,
    "- 标准答案：CircuitService 解析同一块板子的 `.kicad_sch`（确定性解析，置信度 1）",
    "- 识别结果：外部给出的识别草稿 JSON（无 confidence，元素 id 只在草稿内互引）",
    "",
    "## 匹配口径",
    "",
    "- 器件：位号相同；`value` / `partNumber` 在两侧都给出来时必须相同（去空白、不区分大小写）。",
    "- 网络：名称相同（两侧都匿名时按引脚集合）；引脚集合相同，写成 `<位号>.<引脚号>`。",
    "- 准确率 = 匹配数 / 识别出的元素数；召回率 = 匹配数 / 标准答案的元素数；合计是 micro 平均。",
    "",
  ];
  if (kicad.commands.length > 0) {
    lines.push("## 语料生成命令", "", ...kicad.commands.map((command) => `- \`${command}\``), "");
  }

  lines.push(
    "| 板子 | 来源 | 样本 | 器件 准确率 | 器件 召回率 | 网络 准确率 | 网络 召回率 |",
    "| --- | --- | --- | --- | --- | --- | --- |",
  );
  for (const result of results) {
    lines.push(
      `| ${result.boardId} | ${result.source} | ${result.components.recognized}/${result.components.standard} 器件，${result.nets.recognized}/${result.nets.standard} 网络 | ${formatRatio(result.components.precision)} | ${formatRatio(result.components.recall)} | ${formatRatio(result.nets.precision)} | ${formatRatio(result.nets.recall)} |`,
    );
  }
  const total = aggregateResults(results);
  lines.push(
    `| **合计** | | ${results.length} 份识别 | **${formatRatio(total.components.precision)}** | **${formatRatio(total.components.recall)}** | **${formatRatio(total.nets.precision)}** | **${formatRatio(total.nets.recall)}** |`,
    "",
    "## 未能匹配的元素",
    "",
  );
  for (const result of results) {
    const answer = answers.get(result.boardId);
    lines.push(
      `### ${result.boardId} · ${result.source}`,
      "",
      `- 标准答案规模：器件 ${answer.view.components.length}、网络 ${answer.view.nets.length}`,
      ...unmatchedLines("器件漏识别（标准里有、识别没有）", result.components.missing),
      ...unmatchedLines("器件误识别（识别有、标准里没有）", result.components.extra),
      ...unmatchedLines("网络漏识别", result.nets.missing),
      ...unmatchedLines("网络误识别", result.nets.extra),
      "",
    );
  }
  lines.push(
    "## 已知偏差",
    "",
    "- 标准答案只含非电源符号器件；识别草稿若把电源符号（GND/+3V3）当器件提交，会算作器件误识别。",
    "- 器件比对不看引脚数，引脚差异体现在网络比对（引脚集合）上。",
    "- 报告不含模型调用本身：识别草稿由 Agent 会话产出后放进 `--recognitions` 目录。",
    "",
  );
  return lines;
}

/** 未匹配清单：一条一行（清单可能很长），没有未匹配项时写 `—`。 */
function unmatchedLines(label, items) {
  if (items.length === 0) return [`- ${label}：0 项`];
  return [`- ${label}：${items.length} 项`, ...items.map((item) => `  - ${item}`)];
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    console.log(HELP);
    return;
  }
  if (!options.model) throw new Error(`缺少 --model（报告里要写明模型标签）。\n\n${HELP}`);
  if (!options.recognitions) throw new Error(`缺少 --recognitions <目录>。\n\n${HELP}`);
  if (BOARDS.length === 0) throw new Error("没有可评测的板子");

  const kicad = options.skipCorpus
    ? { version: "（--skip-corpus：未探测）", commands: [] }
    : await generateCorpus(options.corpusDir, options.boards);
  const [answers, drafts] = [
    await loadStandardAnswers(options.boards),
    await loadRecognitions(options.recognitions, options.boards),
  ];

  const results = [];
  for (const [key, draft] of [...drafts].sort(([a], [b]) => a.localeCompare(b))) {
    const answer = answers.get(draft.boardId);
    if (!answer) throw new Error(`not_found: 标准答案里没有板子 ${draft.boardId}`);
    const corpusFile = join(options.corpusDir, `${draft.boardId}.${draft.source}`);
    if (!options.skipCorpus && !existsSync(corpusFile))
      throw new Error(
        `not_found: 识别草稿 ${key} 对应语料 ${corpusFile}，先跑语料生成或 --skip-corpus 前手工放置`,
      );
    results.push({
      boardId: draft.boardId,
      source: draft.source,
      ...compareRecognition(answer.view, draft.view),
    });
  }
  if (results.length === 0) throw new Error("not_found: 没有任何识别草稿与所选板子匹配");

  const reportPath = join(options.corpusDir, "recognition-eval-report.md");
  await mkdir(options.corpusDir, { recursive: true });
  await writeFile(
    reportPath,
    `${reportLines({ model: options.model, corpusDir: options.corpusDir, kicad, answers, results }).join("\n")}\n`,
    "utf-8",
  );
  const total = aggregateResults(results);
  console.log(`识别评测完成：${results.length} 份识别，报告写入 ${reportPath}`);
  console.log(
    `器件 precision ${formatRatio(total.components.precision)} / recall ${formatRatio(total.components.recall)}；网络 precision ${formatRatio(total.nets.precision)} / recall ${formatRatio(total.nets.recall)}`,
  );
}

await main().catch((error) => {
  console.error(
    `[circuit-recognition-eval] ${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 1;
});
