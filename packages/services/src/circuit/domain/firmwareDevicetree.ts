import {
  canonicalPortPinCandidates,
  normalizeFirmwareSignal,
  type FirmwarePinDeclaration,
} from "./firmwarePins.js";

/**
 * Zephyr / Linux devicetree 解析（规格 §7.1）：只取确定性可判定的引脚声明。
 *
 * 支持的构造：属性名以 `gpios` / `-gpio` / `-gpios` / `-pins` 结尾，值是 GPIO 描述符
 * `<&gpio<口> <脚> …flags>`；信号名 = 「节点名（有标签用标签）+ 属性前缀」，如
 * `&i2c1 { sda-gpios = <&gpiob 7 …>; }` → `I2C1_SDA`；LED 这类写在子节点里的
 * `led0: led_0 { gpios = <&gpiob 6 …>; }` → `LED0` @ PB6。
 *
 * 不支持的构造（规格 §7.1 已声明，跳过不猜）：`pinctrl-0 = <&label>` 这类间接引用、
 * `NRF_PSEL(...)`、自定义引脚映射属性；数字端口控制器（`&gpio0 12`）按绝对 GPIO 号映射成 `GPIO12`。
 *
 * 实现是自带的轻量 tokenizer + 节点栈，不做完整 DT 语法：注释、字符串、`#…` 预处理行先跳过，
 * 认不出的子结构整体忽略，保证行号与原文一致。纯函数、零 IO。
 */

interface DtToken {
  kind: "ident" | "string" | "other";
  value: string;
  line: number;
}

const PUNCTUATION = new Set(["<", ">", "{", "}", ";", "=", ",", ":", "&", "@", "[", "]"]);

export function parseDevicetreePins(params: {
  path: string;
  text: string;
}): FirmwarePinDeclaration[] {
  const tokens = tokenize(stripPreprocessorLines(params.text));
  const declarations: FirmwarePinDeclaration[] = [];
  const stack: { name: string; label?: string }[] = [];
  let index = 0;
  while (index < tokens.length) {
    const token = tokens[index]!;
    if (token.value === ";") {
      index += 1;
      continue;
    }
    if (token.value === "}") {
      stack.pop();
      index += 1;
      continue;
    }
    // `&{/path/to/node} { … }`：按路径末段取节点名。
    if (token.value === "&" && tokens[index + 1]?.value === "{") {
      index += 2;
      const path: string[] = [];
      while (index < tokens.length && tokens[index]!.value !== "}") {
        path.push(tokens[index]!.value);
        index += 1;
      }
      index += 1;
      if (tokens[index]?.value === "{") {
        stack.push({ name: lastPathSegment(path.join("")) });
        index += 1;
      }
      continue;
    }
    // 头部：属性名或节点名，读到 `{` / `=` / `;` 为止。
    const head: DtToken[] = [];
    while (
      index < tokens.length &&
      tokens[index]!.value !== "{" &&
      tokens[index]!.value !== "=" &&
      tokens[index]!.value !== ";"
    ) {
      head.push(tokens[index]!);
      index += 1;
    }
    const terminator = tokens[index]?.value;
    if (terminator === "{") {
      stack.push(readNodeHeader(head) ?? { name: "" });
      index += 1;
      continue;
    }
    if (terminator === "=") {
      index += 1;
      const value = readPropertyValue(tokens, index);
      index = value.next;
      const node = stack.at(-1);
      if (node)
        collectPinDeclarations({
          node,
          property: head[0]?.value,
          line: head[0]?.line ?? value.tokens[0]?.line ?? 0,
          value,
          declarations,
          path: params.path,
        });
      continue;
    }
    // `;` 与认不出的头部（`/dts-v1/`、`/plugin/` 等）：跳过头部，主循环继续。
  }
  return declarations;
}

/**
 * 属性值收集：`;` / 同级 `}` 结束，`<…>` 组内的分号与花括号不算结束（cell 数组里不出现，
 * 但保持通用以免认错结构）。返回下一个待处理下标。
 */
function readPropertyValue(
  tokens: DtToken[],
  start: number,
): { tokens: DtToken[]; next: number } {
  const collected: DtToken[] = [];
  let angle = 0;
  let index = start;
  while (index < tokens.length) {
    const token = tokens[index]!;
    if (token.value === "<") angle += 1;
    else if (token.value === ">") angle = Math.max(0, angle - 1);
    else if (angle === 0 && (token.value === ";" || token.value === "}")) break;
    collected.push(token);
    index += 1;
  }
  if (tokens[index]?.value === ";") index += 1;
  return { tokens: collected, next: index };
}

function collectPinDeclarations(input: {
  node: { name: string; label?: string };
  property: string | undefined;
  line: number;
  value: { tokens: DtToken[] };
  declarations: FirmwarePinDeclaration[];
  path: string;
}): void {
  const property = input.property;
  if (property === undefined) return;
  const match = /^(?:(.*?)[-_])?(gpios?|pins)$/iu.exec(property);
  if (!match) return;
  const nodeName = (input.node.label ?? input.node.name).replace(/@.*$/u, "").trim();
  const nodeSignal = normalizeFirmwareSignal(nodeName);
  if (nodeSignal === undefined) return;
  // 属性前缀按 `_` 分词已经出现在节点名里时不再重复拼接（`status-led` + `led_gpios` → `STATUS_LED`）。
  const prefix = normalizeFirmwareSignal(match[1] ?? "");
  const signal =
    prefix === undefined || nodeSignal.split("_").includes(prefix)
      ? nodeSignal
      : `${nodeSignal}_${prefix}`;
  for (const spec of readGpioSpecs(input.value.tokens)) {
    const [pin, ...pinAliases] = canonicalPortPinCandidates(spec.port, spec.pin);
    input.declarations.push({
      signal,
      ...(pin !== undefined ? { pin } : {}),
      ...(pinAliases.length > 0 ? { pinAliases } : {}),
      symbol: nodeName,
      file: input.path,
      line: input.line,
    });
  }
}

/** 在 cell 数组里找 `&gpio<口> <脚>` 描述符；`&gpio0` 这类数字端口按绝对 GPIO 号映射。 */
function readGpioSpecs(tokens: DtToken[]): { port: string; pin: string }[] {
  const specs: { port: string; pin: string }[] = [];
  for (let index = 0; index < tokens.length - 2; index += 1) {
    if (tokens[index]!.value !== "&") continue;
    const controller = /^gpio([A-K]|\d{1,2})$/iu.exec(tokens[index + 1]!.value);
    if (!controller) continue;
    const pin = /^\d{1,3}$/u.exec(tokens[index + 2]!.value);
    if (!pin) continue;
    specs.push({ port: controller[1]!, pin: pin[0] });
    index += 2;
  }
  return specs;
}

/** 节点头：`label: name`、`name`、`&ref`、`name@addr`。 */
function readNodeHeader(head: DtToken[]): { name: string; label?: string } | undefined {
  const values = head.map((token) => token.value);
  const colon = values.indexOf(":");
  if (colon > 0 && colon < values.length - 1)
    return { label: values.slice(0, colon).join(""), name: values.slice(colon + 1).join("") };
  const last = values.at(-1);
  if (last === undefined || last.length === 0) return undefined;
  return { name: last };
}

function lastPathSegment(path: string): string {
  return path.replace(/\/$/u, "").split("/").at(-1) ?? "";
}

/** 逐行清空 `#…` 预处理行（保留换行，保证行号不变）。 */
function stripPreprocessorLines(text: string): string {
  return text
    .split("\n")
    .map((line) => (/^\s*#/u.test(line) ? "" : line))
    .join("\n");
}

/** 轻量 tokenizer：跳过注释与字符串内部的空白，保留每个 token 的起始行号。 */
function tokenize(text: string): DtToken[] {
  const tokens: DtToken[] = [];
  let index = 0;
  let line = 1;
  while (index < text.length) {
    const char = text[index]!;
    if (char === "\n") {
      line += 1;
      index += 1;
      continue;
    }
    if (char === "/" && text[index + 1] === "*") {
      index += 2;
      while (index < text.length && !(text[index] === "*" && text[index + 1] === "/")) {
        if (text[index] === "\n") line += 1;
        index += 1;
      }
      index += 2;
      continue;
    }
    if (char === "/" && text[index + 1] === "/") {
      while (index < text.length && text[index] !== "\n") index += 1;
      continue;
    }
    if (/\s/u.test(char)) {
      index += 1;
      continue;
    }
    if (PUNCTUATION.has(char)) {
      tokens.push({ kind: "other", value: char, line });
      index += 1;
      continue;
    }
    if (char === '"') {
      const startLine = line;
      let value = "";
      index += 1;
      while (index < text.length && text[index] !== '"') {
        if (text[index] === "\n") line += 1;
        value += text[index];
        index += 1;
      }
      index += 1;
      tokens.push({ kind: "string", value, line: startLine });
      continue;
    }
    const start = index;
    const startLine = line;
    while (index < text.length && !/[\s"<>{};=,:&@[\]/]/u.test(text[index]!)) index += 1;
    const value = text.slice(start, index);
    if (value.length === 0) {
      index += 1;
      continue;
    }
    tokens.push({ kind: /^[A-Za-z_]/u.test(value) ? "ident" : "other", value, line: startLine });
  }
  return tokens;
}
