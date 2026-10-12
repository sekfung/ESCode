import {
  CIRCUIT_LOW_CONFIDENCE_THRESHOLD,
  type CircuitComponent,
  type CircuitIR,
  type CircuitNet,
  type CircuitPin,
} from "@escode/shared/circuit";

/**
 * 规则判定的 IR 派生事实（规格 §6、§6.2）。
 *
 * 规格 §2 规则 1 要求规则只读 Circuit IR。IR v1 不携带引脚电气类型（power_in / no_connect）、
 * 不携带 `.kicad_sch` 的 no-connect 标记，也不保留同一网络的多个标签名，因此这里的判定用
 * **确定性代理**：引脚名、位号前缀、网络名。代理口径逐条写在下面的注释里，纯函数、零 IO。
 */

export interface CircuitRuleFacts {
  componentById: Map<string, CircuitComponent>;
  netById: Map<string, CircuitNet>;
  /** 具名网络按名字分组（只含 net.name 非空的网络）。 */
  netsByName: Map<string, CircuitNet[]>;
  /** 直接参与设施：元素 id → 是否低置信度。 */
  isLowConfidenceElement(elementId: string): boolean;
}

export function buildCircuitRuleFacts(ir: CircuitIR): CircuitRuleFacts {
  const componentById = new Map(ir.components.map((component) => [component.id, component]));
  const netById = new Map(ir.nets.map((net) => [net.id, net]));
  const netsByName = new Map<string, CircuitNet[]>();
  const lowConfidence = new Set<string>();
  for (const element of [...ir.components, ...ir.nets, ...ir.blocks])
    if (element.origin.confidence < CIRCUIT_LOW_CONFIDENCE_THRESHOLD) lowConfidence.add(element.id);
  for (const net of ir.nets) {
    if (net.name === undefined) continue;
    const list = netsByName.get(net.name);
    if (list) list.push(net);
    else netsByName.set(net.name, [net]);
  }
  return {
    componentById,
    netById,
    netsByName,
    isLowConfidenceElement: (elementId) => lowConfidence.has(elementId),
  };
}

/** 器件上的指定引脚（引脚号在 IR 内唯一）。 */
export function pinOf(
  component: CircuitComponent,
  pinNumber: string,
): CircuitPin | undefined {
  return component.pins.find((pin) => pin.number === pinNumber);
}

/**
 * 电阻引脚除自身外连到的网络 id：用来判断上拉 / 去耦这类「电容电阻另一端接在哪」的关系。
 * 引脚名在 IR 里可能缺失，所以只看引脚号与 netId。
 */
export function otherPinNetIds(
  component: CircuitComponent,
  pin: CircuitPin,
  facts: CircuitRuleFacts,
): string[] {
  const netIds = new Set<string>();
  for (const candidate of component.pins) {
    if (candidate.number === pin.number) continue;
    if (candidate.netId) netIds.add(candidate.netId);
  }
  return [...netIds].filter((netId) => facts.netById.has(netId));
}

/**
 * 电源输入引脚名（规格 §6 `undriven-power` 的「电源输入引脚」、`missing-decoupling` 的「IC 电源引脚」）。
 * IR 没有电气类型，这里用 KiCad 库符号的引脚命名约定：电源网名（`+3V3` / `3.3V` / `1V8` / `5V`）
 * 与 `VCC` / `VDD` 族。**不含** GND / VSS 族（地是回路，不是电源输入），避免整板地网被误判。
 */
export function isPowerInputPinName(name: string | undefined): boolean {
  if (name === undefined) return false;
  const value = name.trim();
  if (/^[+-]?\d+(?:\.\d+)?V\d*$/iu.test(value)) return true;
  return /^(?:V(?:CC|DD|BAT|IN|BUS|EE|REF|DDA|DDIO|DDP|DD_?CORE)|AVDD|DVDD)$/iu.test(value);
}

/**
 * 地网名：`missing-decoupling` 要求去耦电容另一端落在地网；`i2c-missing-pullup` 不接受上拉到地。
 */
export function isGroundNetName(name: string | undefined): boolean {
  if (name === undefined) return false;
  return /^(?:GND|AGND|DGND|GNDA|VSS|VSSA|0V|EARTH)$/iu.test(name.trim());
}

/**
 * 非地的电源网名：上拉电阻的目标轨。
 * 复用电源输入引脚名的命名约定（`+3V3` / `VCC` / `VBUS` …），再排除地网名。
 */
export function isPowerNetName(name: string | undefined): boolean {
  return !isGroundNetName(name) && isPowerInputPinName(name);
}

/** 无源件位号前缀：不能作为电源驱动源的器件（电阻、电容、电感、磁珠、二极管、晶振、保险丝、跳线、测试点）。 */
export function isPassiveRef(ref: string): boolean {
  return /^(?:C|L|FB|F|D|Y|X|RN?|TP|JP)\d/iu.test(ref.trim());
}

/** 连接器与电池类位号前缀：任何引脚都算「外部电源/信号来源」。 */
export function isConnectorRef(ref: string): boolean {
  return /^(?:J|P|CN|USB|PS|BT|BAT)\d/iu.test(ref.trim());
}

/** IC 位号前缀：`missing-decoupling` 只检查 IC。 */
export function isIcRef(ref: string): boolean {
  return /^(?:IC|U)\d/iu.test(ref.trim());
}

export function isResistorRef(ref: string): boolean {
  return /^RN?\d/iu.test(ref.trim());
}

export function isCapacitorRef(ref: string): boolean {
  return /^C\d/iu.test(ref.trim());
}

/**
 * NC（no-connect）引脚名：IR 里没有 no-connect 标记，只能用引脚名代理（规格 §6 的「非 NC 引脚」）。
 * 无法识别的 NC 引脚仍会被报出——这是 §6.2 声明的 P1 边界。
 */
export function isNoConnectPinName(name: string | undefined): boolean {
  if (name === undefined) return false;
  return /^(?:NC|N\/C|DNC)[_\-. ]?\d*$/iu.test(name.trim());
}

/** 解析器给未命名网络起的名字（`Net-(R1-Pad2)`）：不是设计里写下的信号名，不参与固件比对。 */
export function isSynthesizedNetName(name: string | undefined): boolean {
  if (name === undefined) return true;
  const value = name.trim();
  return value.length === 0 || value.startsWith("Net-(");
}

/**
 * I²C 信号网名：`SDA` / `SCL` 结尾（允许 `I2C1_SDA`、`I2C-SCL`、`TWI1_SCL`）。
 * 不匹配 `SCLK` / `SDIO` / `XDA` / `XCL` 这类其它接口写法，避免张冠李戴。
 */
export function isI2cNetName(name: string | undefined): boolean {
  if (name === undefined) return false;
  return /(?:^|[_\-.])S(?:DA|CL)$/iu.test(name.trim());
}
