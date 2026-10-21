# 桌面构建期可选能力

## 目标

构建期可选能力由配置决定是否启用，未配置时关闭。当前只剩自动更新：

| 能力     | 配置项                                   | 关闭时的行为                            |
| -------- | ---------------------------------------- | --------------------------------------- |
| 自动更新 | `ESCODE_AUTO_UPDATE`（`1`/`true`/`yes`） | 与 Preview 形态相同，不检查、不下载更新 |

产品埋点、ARMS RUM 与 CLI Agent OTLP 已在 2026-10-21 删除，不再解析端点或注入遥测配置。

## 配置来源与优先级

1. 构建进程的环境变量。显式设置为空字符串表示关闭。
2. `config/private/build-defaults.json`。只存在于内部仓库，开源导出不包含。
3. 以上都没有时关闭。

## 所有者与数据流

```
环境变量 / config/private/build-defaults.json
        │ scripts/build-time-config.mjs（唯一解析入口）
        ▼
desktop tsup（main、host、preload）与 desktop vite（renderer）的 define
        │ __ESCODE_AUTO_UPDATE_ENABLED__
        ▼
@escode/shared env 常量 ── 运行时代码只读这些常量
```

- 未注入 define 的环境（测试、其他 bundle）得到关闭值。
- 自动更新在 Desktop Main 启动时判定：`ESCODE_PRODUCT_FLAVOR === "production" && ESCODE_AUTO_UPDATE_ENABLED`。

## 验收

1. 没有私有默认配置、也没有环境变量时，自动更新关闭。
2. 环境变量优先于私有默认配置，显式空字符串可以关闭。
3. 类型检查、Lint 与相关单测通过。
