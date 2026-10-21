# Monitoring 文档入口

本目录保留本地诊断能力。产品埋点、ARMS RUM、CLI Agent OTLP、Renderer user action trace
与本地 TTFT 导出已在 2026-10-21 内网产品决定中整体删除，不再作为构建能力或 M3 迁移缺口。

## 本地诊断

- [进程内存本地诊断日志](./memory-diagnostics-log.md)
- [会话停止来源诊断](./session-stop-source-diagnostics.md)

## 说明

- `deviceMid` / `X-Device-Mid` 仍用于 ESCode endpoint 身份与计费，不是遥测上报。
- 本地 Resource Manager、崩溃归档与 usage 统计继续保留。
