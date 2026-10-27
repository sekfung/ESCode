// bench-escode-app 的报告落盘（从主脚本拆出，保证主文件留在 lint max-lines 内）。
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  isWindows,
  renderTable,
  roleBreakdownTable,
  summaryHeaders,
  summaryRows,
} from "./bench-escode-app-sampling.mjs";

export async function writeBenchReport({
  outDir,
  summary,
  selectedKeys,
  reps,
  scenario,
  installedApp,
  began,
}) {
  await mkdir(outDir, { recursive: true });
  const now = new Date();
  const stamp = now.toISOString().slice(0, 10);
  const summaryPath = join(outDir, "summary.json");
  const reportPath = join(outDir, `report-${stamp}.md`);
  await writeFile(summaryPath, JSON.stringify(summary, null, 2));
  await writeFile(
    reportPath,
    [
      "# App 级性能测量",
      "",
      `- 时间：${now.toISOString()}；配置：${selectedKeys.join(" / ")}；交错 ${reps} 轮；场景：${scenario}`,
      `- 口径：${isWindows ? "Windows WorkingSet64（工作集，footprint 近似）+ PrivateMemorySize64（私有提交）" : "RSS（macOS footprint 口径待接入）"}；CPU=采样窗口内进程 CPU 时间差折算的单核百分比`,
      `- 安装包：${installedApp ?? "否（dev 检出）"}；就绪：${installedApp ? "主进程日志（数据库 ready + 首屏 did-finish-load）" : "进程树（electron 主进程 + renderer）"}`,
      "- 方法学：交错执行、每配置取中位数、每轮隔离 data dir（A/B 交替复用同一 data dir 会互相污染存储与会话）、本 harness 不建立调试连接",
      `- ${installedApp ? "安装包（ASAR）" : "开发检出（无 ASAR 与签名）"}口径；原始数据见 summary.json；总耗时 ${((Date.now() - began) / 1000).toFixed(0)}s`,
      "",
      "```",
      renderTable(summaryHeaders, summaryRows(summary)),
      "```",
      "",
      ...Object.entries(summary).flatMap(([key, value]) =>
        value.startupPhases
          ? [`冷启动阶段（${key}，T0 进程创建 → T5 React 首次 commit）：`, "", "```", value.startupPhases, "```", ""]
          : [],
      ),
      "角色分解（App = 应用进程；非 App = dev 构建链/系统壳，不计入 §5「全 App」口径）：",
      "",
      "```",
      roleBreakdownTable(summary),
      "```",
      "",
      "末次采样逐进程明细（末次运行；role=other 需人工判读并回填 classify）：",
      "",
      "```",
      ...Object.entries(summary).flatMap(([key, value]) => [
        `# ${key}`,
        renderTable(
          ["pid", "名称", "角色", isWindows ? "工作集 MiB" : "RSS MiB", "私有提交 MiB", "命令行"],
          (value.lastProcessRows ?? []).map((row) => [
            row.pid,
            row.name,
            row.role,
            isWindows ? row.workingSetMiB : row.rssMiB,
            row.privateMiB,
            row.commandLine,
          ]),
        ),
        "",
      ]),
      "```",
      "",
      "> 已知缺口：多 workspace（N=1/4/8）、首条消息首段、4 路流式 CPU 需要 mock SSE 与会话驱动；" +
        "macOS footprint 口径待接入。安装包模式已覆盖 A=Node Host+Node CLI、B=Node Host+Rust CLI、" +
        "C=Rust Host+内嵌 runtime 的冷启动/空闲内存/退出回收对照；退出仍是整树 SIGKILL 回收，" +
        "不是 8 workspace 优雅退出口径。",
      "",
    ].join("\n"),
  );
  return { summaryPath, reportPath };
}
