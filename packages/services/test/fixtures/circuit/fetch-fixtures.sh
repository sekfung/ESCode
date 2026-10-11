#!/usr/bin/env bash
# 下载电路识别测试用的 3 块开源 KiCad 板子（设计文件按上游字节原样保存，不做改写）。
#
# 用法：
#   bash packages/services/test/fixtures/circuit/fetch-fixtures.sh
#
# 来源仓库、许可证与文件清单见同目录 README.md。脚本内固化 commit sha，
# 重复执行会得到逐字节相同的文件；末尾会打印 sha256 便于核对。
set -euo pipefail

DEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAW_BASE="https://raw.githubusercontent.com"

fetch() {
  local url="$1"
  local out="$2"
  mkdir -p "$(dirname "${DEST_DIR}/${out}")"
  curl -fsSL "${url}" -o "${DEST_DIR}/${out}"
  printf '%s\t%s\n' "$(wc -c < "${DEST_DIR}/${out}" | tr -d ' ')" "${out}"
}

# 1) MCU 最小系统：Raspberry Pi RP2040 Minimal Design Example（KiCad 7），BSD-3-Clause
RP2040_REPO="tommy-gilligan/RP2040-minimal-design"
RP2040_SHA="7a3e5234447a9e01624c6a8de12d510f9e0161a7"
for name in RP2040_minimal.kicad_sch RP2040_minimal.kicad_pcb RP2040_minimal.kicad_pro; do
  fetch "${RAW_BASE}/${RP2040_REPO}/${RP2040_SHA}/${name}" "rp2040-minimal/${name}"
done

# 2) 电源板：USB-C 供电的 3.3V/3A 降压型 DC-DC（KiCad 7），MIT
BUCK_REPO="maudeve-it/3.3V-3A-step-down-converter-powered-by-USB-C"
BUCK_SHA="86388564284f5de664711cc6cb92e92855deefb2"
# 上游目录与文件名含空格，需按 URL 规则编码为 %20
for name in "USB power port.kicad_sch" "USB power port.kicad_pcb" "USB power port.kicad_pro"; do
  fetch "${RAW_BASE}/${BUCK_REPO}/${BUCK_SHA}/KiCad/${name// /%20}" "usbc-buck-3v3/${name}"
done

# 3) I2C 传感器板：MPU-6050 六轴 IMU breakout（KiCad 6），MIT
IMU_REPO="racerxdl/kicad-board-collection"
IMU_SHA="8477c32b6a499361b55bfec1312e5a8a899caf05"
for name in MPU6050.kicad_sch MPU6050.kicad_pcb MPU6050.kicad_pro; do
  fetch "${RAW_BASE}/${IMU_REPO}/${IMU_SHA}/MPU6050/${name}" "mpu6050-imu/${name}"
done

echo
echo "已下载设计文件，sha256："
sha256sum "${DEST_DIR}"/rp2040-minimal/* "${DEST_DIR}"/usbc-buck-3v3/* "${DEST_DIR}"/mpu6050-imu/*
