// 生成 Rust Host 契约清单的 **TS 服务面全集**（评审 A13，docs/reports/rust-unified-host-audit-2026-10-15.md）。
//
// 原清单只手工登记了 9 个 channel，「清单与源码一致」因此只覆盖被登记的子集——未登记 channel 的缺口
// 不可见。这里用 TypeScript 编译器 API 从 `packages/services` 源码里解析每个
// `createServiceDescriptor<T>(channel)`：channel 名取实参的字面量类型，成员取 `T` 的全部可调用属性
// （方法与 `onX`/`onDynamicX` 事件；`ProxyChannel.fromService` 暴露的正是这些）。产物是排序后的
// `(channel, member)` 表，Rust 侧据此把未实现成员一律标为 Pending。
//
// 用法：`node scripts/generate-escode-host-service-inventory.mjs [--check]`（`--check` 已进 `pnpm check:escode-host`）。
import { readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const configPath = join(root, "packages/services/tsconfig.json");
const outputPath = join(root, "apps/escode-host/crates/host-proto/src/inventory_ts.rs");

const config = ts.getParsedCommandLineOfConfigFile(
  configPath,
  {},
  {
    ...ts.sys,
    onUnRecoverableConfigFileDiagnostic(diagnostic) {
      throw new Error(ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"));
    },
  },
);
if (!config) throw new Error(`cannot parse ${configPath}`);
const program = ts.createProgram({ rootNames: config.fileNames, options: config.options });
const checker = program.getTypeChecker();

/** @type {Map<string, Set<string>>} */
const members = new Map();
for (const source of program.getSourceFiles()) {
  if (source.isDeclarationFile || !source.fileName.includes("/packages/services/src/")) continue;
  visit(source);
}

function visit(node) {
  if (
    ts.isCallExpression(node) &&
    ts.isIdentifier(node.expression) &&
    node.expression.text === "createServiceDescriptor" &&
    node.typeArguments?.length === 1 &&
    node.arguments.length === 1
  ) {
    record(node);
  }
  ts.forEachChild(node, visit);
}

function record(call) {
  const channelType = checker.getTypeAtLocation(call.arguments[0]);
  if (!channelType.isStringLiteral()) {
    const where = call.getSourceFile().fileName.replace(`${root}/`, "");
    throw new Error(`${where}: descriptor channel is not a string literal`);
  }
  const channel = channelType.value;
  const serviceType = checker.getTypeFromTypeNode(call.typeArguments[0]);
  const names = members.get(channel) ?? new Set();
  for (const property of checker.getPropertiesOfType(serviceType)) {
    const type = checker.getTypeOfSymbolAtLocation(property, call);
    // 可选成员的类型是 `F | undefined`：取非空部分再看调用签名。
    const callable = checker.getNonNullableType(type).getCallSignatures().length > 0;
    if (callable) names.add(property.getName());
  }
  members.set(channel, names);
}

const channels = [...members.entries()]
  .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
  .map(([channel, names]) => [channel, [...names].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0))]);
const memberCount = channels.reduce((sum, [, names]) => sum + names.length, 0);

// 每个 channel 一行（守住 Rust 侧单文件 400 行门禁；`#[rustfmt::skip]` 保持原样）。
const lines = [
  "//! TS 服务面全集（`createServiceDescriptor<T>(channel)` × `T` 的可调用成员），",
  "//! 由 `scripts/generate-escode-host-service-inventory.mjs` 从 `packages/services` 源码生成，勿手改。",
  "",
  "/// `(channel, members)`，按 channel、member 字节序排序。",
  "#[rustfmt::skip]",
  "pub const TS_SERVICE_CHANNELS: &[(&str, &[&str])] = &[",
  ...channels.map(
    ([channel, names]) =>
      `    (${JSON.stringify(channel)}, &[${names.map((name) => JSON.stringify(name)).join(", ")}]),`,
  ),
  "];",
  "",
];
const output = lines.join("\n");

if (process.argv.includes("--check")) {
  const current = await readFile(outputPath, "utf8").catch(() => "");
  if (current !== output) {
    console.error(
      `${outputPath.replace(`${root}/`, "")} is stale: run node scripts/generate-escode-host-service-inventory.mjs`,
    );
    process.exit(1);
  }
  console.log(`Rust Host service inventory: OK (${members.size} channels, ${memberCount} members)`);
} else {
  await writeFile(outputPath, output);
  console.log(`wrote ${memberCount} members across ${members.size} channels`);
}
