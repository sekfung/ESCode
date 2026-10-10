import assert from "node:assert/strict";
import test from "node:test";
import {
  createDesktopNativePackagePrunePatterns,
  findDesktopNativePackageViolations,
  resolveSerialportPrebuildKey,
} from "./desktop-native-package-policy.mjs";

test("serialport prebuild 目录按目标平台映射，macOS 使用 universal 目录", () => {
  assert.equal(resolveSerialportPrebuildKey("darwin-arm64"), "darwin-x64+arm64");
  assert.equal(resolveSerialportPrebuildKey("darwin-x64"), "darwin-x64+arm64");
  assert.equal(resolveSerialportPrebuildKey("linux-x64"), "linux-x64");
  assert.equal(resolveSerialportPrebuildKey("win32-arm64"), "win32-arm64");
});

test("裁剪规则排除非目标平台的 serialport prebuild 与 musl 变体", () => {
  const patterns = createDesktopNativePackagePrunePatterns("linux-x64");
  assert.ok(patterns.includes("!node_modules/@serialport/bindings-cpp/prebuilds/win32-x64/**"));
  assert.ok(patterns.includes("!node_modules/@serialport/bindings-cpp/prebuilds/android-arm/**"));
  assert.ok(patterns.includes("!node_modules/@serialport/bindings-cpp/prebuilds/*/*.musl.node"));
  assert.ok(!patterns.includes("!node_modules/@serialport/bindings-cpp/prebuilds/linux-x64/**"));
});

test("打包产物中非目标平台的 serialport prebuild 视为违规", () => {
  const violations = findDesktopNativePackageViolations(
    [
      {
        packState: "unpack",
        path: "/node_modules/@serialport/bindings-cpp/prebuilds/win32-x64/@serialport+bindings-cpp.node",
      },
      {
        packState: "unpack",
        path: "/node_modules/@serialport/bindings-cpp/prebuilds/linux-x64/@serialport+bindings-cpp.glibc.node",
      },
    ],
    "linux-x64",
  );
  assert.equal(violations.length, 1);
  assert.match(violations[0], /serialport/);
});

test("目标平台的 serialport native 必须 unpack", () => {
  const violations = findDesktopNativePackageViolations(
    [
      {
        packState: "pack",
        path: "/node_modules/@serialport/bindings-cpp/prebuilds/darwin-x64+arm64/@serialport+bindings-cpp.node",
      },
    ],
    "darwin-arm64",
  );
  assert.equal(violations.length, 1);
  assert.match(violations[0], /app\.asar/);
});
