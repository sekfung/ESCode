# 电路测试数据（开源 KiCad 板子）

本目录是电路分析功能的离线测试数据，按 `docs/circuit/README.md` 10.1 的要求存放 3 块**许可宽松**的开源
KiCad 板子的设计文件：

- `rp2040-minimal`：MCU 最小系统
- `usbc-buck-3v3`：电源板（USB-C 供电的降压型 DC-DC）
- `mpu6050-imu`：I²C 传感器板

**未使用任何真实客户的设计**，3 块板全部来自公开的开源硬件项目；文件按上游字节原样保存，未做任何改写。

## 来源与许可

| 板子             | 用途分类                 | 来源仓库                                                                   | 下载 URL（精确到文件）                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | 许可证       | LICENSE 文件 URL                                                                                                                 | commit sha                                 | 本目录文件（字节）                                                                                               |
| ---------------- | ------------------------ | -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ | ---------------------------------------------------------------------------------------------------------------- |
| `rp2040-minimal` | MCU 最小系统             | https://github.com/tommy-gilligan/RP2040-minimal-design                    | https://raw.githubusercontent.com/tommy-gilligan/RP2040-minimal-design/7a3e5234447a9e01624c6a8de12d510f9e0161a7/RP2040_minimal.kicad_sch<br>https://raw.githubusercontent.com/tommy-gilligan/RP2040-minimal-design/7a3e5234447a9e01624c6a8de12d510f9e0161a7/RP2040_minimal.kicad_pcb<br>https://raw.githubusercontent.com/tommy-gilligan/RP2040-minimal-design/7a3e5234447a9e01624c6a8de12d510f9e0161a7/RP2040_minimal.kicad_pro                                                                                        | BSD-3-Clause | https://github.com/tommy-gilligan/RP2040-minimal-design/blob/7a3e5234447a9e01624c6a8de12d510f9e0161a7/LICENSE.txt                | `7a3e5234447a9e01624c6a8de12d510f9e0161a7` | `RP2040_minimal.kicad_sch` (176040)<br>`RP2040_minimal.kicad_pcb` (551088)<br>`RP2040_minimal.kicad_pro` (15840) |
| `usbc-buck-3v3`  | 电源板（降压型 DC-DC）   | https://github.com/maudeve-it/3.3V-3A-step-down-converter-powered-by-USB-C | https://raw.githubusercontent.com/maudeve-it/3.3V-3A-step-down-converter-powered-by-USB-C/86388564284f5de664711cc6cb92e92855deefb2/KiCad/USB%20power%20port.kicad_sch<br>https://raw.githubusercontent.com/maudeve-it/3.3V-3A-step-down-converter-powered-by-USB-C/86388564284f5de664711cc6cb92e92855deefb2/KiCad/USB%20power%20port.kicad_pcb<br>https://raw.githubusercontent.com/maudeve-it/3.3V-3A-step-down-converter-powered-by-USB-C/86388564284f5de664711cc6cb92e92855deefb2/KiCad/USB%20power%20port.kicad_pro | MIT          | https://github.com/maudeve-it/3.3V-3A-step-down-converter-powered-by-USB-C/blob/86388564284f5de664711cc6cb92e92855deefb2/LICENSE | `86388564284f5de664711cc6cb92e92855deefb2` | `USB power port.kicad_sch` (165033)<br>`USB power port.kicad_pcb` (357306)<br>`USB power port.kicad_pro` (11165) |
| `mpu6050-imu`    | I²C 传感器板（6 轴 IMU） | https://github.com/racerxdl/kicad-board-collection                         | https://raw.githubusercontent.com/racerxdl/kicad-board-collection/8477c32b6a499361b55bfec1312e5a8a899caf05/MPU6050/MPU6050.kicad_sch<br>https://raw.githubusercontent.com/racerxdl/kicad-board-collection/8477c32b6a499361b55bfec1312e5a8a899caf05/MPU6050/MPU6050.kicad_pcb<br>https://raw.githubusercontent.com/racerxdl/kicad-board-collection/8477c32b6a499361b55bfec1312e5a8a899caf05/MPU6050/MPU6050.kicad_pro                                                                                                    | MIT          | https://github.com/racerxdl/kicad-board-collection/blob/8477c32b6a499361b55bfec1312e5a8a899caf05/LICENSE                         | `8477c32b6a499361b55bfec1312e5a8a899caf05` | `MPU6050.kicad_sch` (60084)<br>`MPU6050.kicad_pcb` (72135)<br>`MPU6050.kicad_pro` (9334)                         |

每块板的 LICENSE 原文前两行有效内容（原文第 2 行为空行，版权行紧随其后）：

- `rp2040-minimal`：`BSD 3-Clause License` / `Copyright © 2020-2022 Raspberry Pi Ltd (formerly Raspberry Pi (Trading) Ltd.), 2023 Tommy Gilligan`
- `usbc-buck-3v3`：`MIT License` / `Copyright (c) 2023 Mauro De Vecchi`
- `mpu6050-imu`：`MIT License` / `Copyright (c) 2022 Lucas Teske`

## 板子说明

### rp2040-minimal（MCU 最小系统）

Raspberry Pi 官方 “RP2040 Minimal Design Example” 的 KiCad 7 版本（原理图 version 20230121、PCB
version 20221018）。器件规模：原理图 60 个放置符号（其中 34 个为非电源符号）、PCB 34 个封装、53 个网络；
核心器件为 RP2040、W25Q128JVS QSPI Flash、12 MHz 无源晶振（ABLS-12.000MHZ-B4-T）、NCP1117-3.3
LDO、USB Micro-B 与两排 18 pin 排针，属于典型的 MCU 最小系统结构。含 PCB 文件（双层板 F.Cu/B.Cu，
带铺铜，因此 `.kicad_pcb` 体积最大）；原理图为单页，无层次化子图。

### usbc-buck-3v3（电源板）

USB-C 受电、输出 5 V 与 3.3 V（最大 3 A）的降压型 DC-DC 电源板，KiCad 7
（原理图 version 20230121、PCB version 20221018）。器件规模：原理图 84 个放置符号（其中 55 个为非
电源符号）、PCB 53 个封装、39 个网络；核心为 TPS54331 降压变换器 + 3.3 µH 电感，配
AO3400A/AO3401A MOSFET、SS34 肖特基、LM358、SS8050 以及多路电阻分压做电压选择/指示，另有
7 个测试点（TP1–TP7）。含 PCB 文件（双层板 F.Cu/B.Cu）；原理图同样为单页。

### mpu6050-imu（I²C 传感器板）

MPU-6050 六轴加速度计/陀螺仪的 breakout 板，KiCad 6
（原理图 version 20211123、PCB version 20211014）。器件规模：原理图 26 个放置符号（其中 14 个为非
电源符号）、PCB 14 个封装、16 个网络；核心为 MPU-6050 与 TC2185-3.3VxCTTR LDO，引出 8 pin 带固定
孔排针与状态 LED。网络名含 `SDA`/`SCL`/`XDA`/`XCL`/`AUX_SDA`/`AUX_SCL`/`AD0`，可确认 I²C 接口与
地址选择引脚，是三者中结构最简单的一块，适合作为解析的最小样例。含 PCB 文件
（双层板 F.Cu/B.Cu）；原理图为单页。

## 获取方式

下载脚本：`./fetch-fixtures.sh`（在仓库根目录执行 `bash packages/services/test/fixtures/circuit/fetch-fixtures.sh`
亦可）。脚本内固化了上表的 commit sha，重复执行会得到完全相同的文件。

## 字节一致性与换行

上游 3 个仓库的设计文件本身使用 CRLF 换行，而本仓库在 Windows 上 `core.autocrlf=true`，提交时会把
CRLF 规范化成 LF、使 fixture 与上游不再逐字节一致。因此本目录放置了 `.gitattributes`，对
`*.kicad_sch` / `*.kicad_pcb` / `*.kicad_pro` 声明 `-text`，禁止换行转换（与仓库根 `.gitattributes`
中 PDF、上游许可证原文的处理方式一致）。

校验方法（blob sha 与上游一致）：

```bash
git hash-object --no-filters packages/services/test/fixtures/circuit/rp2040-minimal/RP2040_minimal.kicad_sch
# 330ed51f73d857e8d10f68a32eedccd660bf8525  ← 上游 blob sha
```

各文件的 sha256 与上游 blob sha：

| 文件                                      | sha256                                                             | 上游 blob sha                              |
| ----------------------------------------- | ------------------------------------------------------------------ | ------------------------------------------ |
| `rp2040-minimal/RP2040_minimal.kicad_sch` | `048b1708e043f81b12365577bf4f9340e1eddadd5acfcecd159fa26acc005f6d` | `330ed51f73d857e8d10f68a32eedccd660bf8525` |
| `rp2040-minimal/RP2040_minimal.kicad_pcb` | `0e8a6603cfc8e4a252fb257f3c20554856bcc0a1f43bae133cc5f21735bbb0c2` | `478cba970ba903735b0cbcfd646dc0cc7675f043` |
| `rp2040-minimal/RP2040_minimal.kicad_pro` | `b4b6414140da58dfdd22dd799b6fcd28d8fbb9b7ed9dffbceb3adc0eeb8ec5ef` | `10bea0db33090a613ab89113af97f05a15eea198` |
| `usbc-buck-3v3/USB power port.kicad_sch`  | `48b1bb95c0d4bbdd448748b8de4d24d2fcec3197b3de8c05a11f445b63adc192` | `f7ba405fece986b0dac65be7d9b5acf4f2d876dc` |
| `usbc-buck-3v3/USB power port.kicad_pcb`  | `3e505ad5aacd4adc550b58d184284f18223fb951053fa78dab5ab1636a7e6112` | `6bd9977100b71cdd8a0317d9cfec42971357049e` |
| `usbc-buck-3v3/USB power port.kicad_pro`  | `9ff260526a3289bfb8d67dccb63a3b7496e4e3a45dd730aa87cfe416adfee69c` | `f61dab0c213e3faf466dc1138a230e1ff60f5166` |
| `mpu6050-imu/MPU6050.kicad_sch`           | `7298ebef901dbe1fc8b2193c650f55fbfcfaca24fc106df0969ca54053681f6d` | `bdc06e4d3a67f03524e805eeb25f56435aba02ef` |
| `mpu6050-imu/MPU6050.kicad_pcb`           | `327f81c96d3d11f2bb8bf8dc76c0bf6d55ab358e93d1e12aa1fae0632790915b` | `c2e2731edf60b5d3d8ba29e8378ed8e376c4f07d` |
| `mpu6050-imu/MPU6050.kicad_pro`           | `461b737a7c5229095014c204597bc63412040da88ce3ade8cafc6f3f5a4d6dfb` | `796cb03371c7b489aa8930a94b0ad60d1a0b2adc` |

## 收录范围

只收录设计文件（`.kicad_sch`、`.kicad_pcb`、`.kicad_pro`）。上游仓库中的 3D 模型（`.step`/`.wrl`）、
gerber、钻孔文件、BOM/CPL、PDF、图片、缓存文件（`*-cache.lib`、`fp-info-cache`）与文档均未收录；
上游只包含 KiCad 5 legacy 格式（`EESchema Schematic File`）或 Altium/其它格式的板子也未收录。
单文件均小于 1 MB（最大 551088 字节），单块板设计文件总量 0.14–0.71 MiB（141553–742968 字节）。

## 手工构造的 fixture（本仓库原创）

`firmware/` 下的文件是**本仓库自己手写**的最小样例，没有从任何第三方项目复制，
不涉及第三方许可证；上面 3 块开源板子保持上游字节原样，未做任何改动。

### `firmware/`：固件比对样例（§10.1）

- `schematic.kicad_sch`：手写的 STM32 最小系统，U1 的引脚名为 `PA2` / `PA3` / `PA4` / `PA9` / `PB6` / `PB7`
  （**故意没有 PB8**），各自接一个具名网络；VDD / VSS 接 +3V3 / GND。
- `board.dts`：devicetree 片段，`led0`→PB6、`led1`→PA9、`uart2.tx-gpios`→PA2、`i2c1.sda-gpios`→PB7、
  `i2c1.scl-gpios`→PB8。
- `board.ioc`：STM32CubeMX 片段，PA2 / PA3 / PB6 用 `.GPIO_Label` 给出信号名，PB7 用 `.Signal=I2C1_SDA`，
  PB8 是没写标签的通用 GPIO（解析时应跳过）。
- `board_pins.h`：CubeMX 风格的 `_PIN` + `_GPIO_Port` / `_PORT` 宏（含一个 `LL_GPIO_PIN_8`）。

三份固件样例**故意埋了 4 处引脚不一致**（与 `schematic.kicad_sch` 对照）：

1. `led1` 声明在 PA9，原理图上 PA9 接的是 LED2 → `pin-mismatch`；
2. `I2C1_SCL` 声明在 PB8，原理图上没有 PB8 → `firmware-only`；
3. 原理图上的 PA4（UART2_CTS）在固件里没有声明 → `schematic-only`；
4. `I2C1_SDA` 在 `board.dts` 是 PB7、在 `board_pins.h` 是 PB8 → `peripheral-conflict`（并各带出
   一条 `firmware-only`）。

断言见 `packages/services/test/circuit/circuitFirmware.test.ts`；断言里的行号以这些文件当前内容为准，
改动 fixture 时要同步更新。

## 已排除的候选（许可证核实记录）

以下项目曾作为候选，实际下载并核对 LICENSE 原文后排除，记录备查：

- CERN-OHL（非要求列表内的许可证）：`fm4dd/pmod-bme280`（CERN-OHL v1.2）、
  `bluerobotics/ping-dev-kit`、`TobleMiner/DC-UPS`、`theta-machines/DEV-G2-NANO`（均 CERN-OHL-P v2）、
  `bitaxeorg/bitaxeGamma`（CERN-OHL-S v2）。
- 传染性或非宽松许可：`wagiminator/Power-Boards`（CC-BY-SA 3.0）、
  `NoBlackBoxes/LastBlackBox`（CC-BY-NC-SA 4.0）、`CoreElectronics/CE-PiicoDev-IMU-MPU6050`（硬件
  CC-BY-SA 4.0，仅代码 MIT）、`wilsondr9999/1016-Breadboard-Power-Supply`（AGPL-3.0）、
  `MicroType-Engineering/MTE_USB_HUB`（GPL-2.0）。
- 无许可证或 license 不明：`VladimirP1/esplog-hardware`、`petersn/circuitsnippets`、`lessthan00/MPU6050`、
  `loxodes/vna`、`hcmos/open_rover-pwr_pcb`、`AirchipTeknoloji/*`、`TeknoTrek/7805-Voltage-Regulator`、
  `bask185/general_schematics`、`tomek-szczesny/OpenUPS`、`PocketConcepts/Pocket_PSU`。
- 格式不符合要求（无 `.kicad_sch`）：`pms67/STM32F4-Reference-PCB`、`pms67/BananaSchplit`
  （KiCad 5 legacy `.sch`）、`pms67/BananaSchplit-MiniUSBC`、`pms67/MPU6050_Breakout`（Altium）、
  `azerimaker/olmBoard`、`futureshocked/KicadLikeAPro-Project-1-BreadboardPowerSupply`、
  `promesoft/KicadJE-EuroPowerSupply`（均为 legacy `.sch`）。
- KiCad 版本超出 6/7/8：`ExTc-aShiSh/buck-converter-kicad-design`、`anujthakur512/Buck-Converter-5V`、
  `Vulpini-cmd/LBC05`（KiCad 9）、`ykupas/DCDC-Buck-KiCAD`（KiCad 10）。
