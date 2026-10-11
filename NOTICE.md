# ESCode

Copyright 2026 ESCode contributors.

## 来源与许可

ESCode 是 ZCode 的衍生作品。ZCode 由 Z.AI Co., Ltd 以 Apache License 2.0 开源（Copyright 2026 Z.AI Co., Ltd），本仓库继续以 Apache License 2.0 分发，许可证全文见根目录 [`LICENSE`](LICENSE)。

上游 ZCode 的 `NOTICE.md`（含功能、数据与第三方组件声明）逐字保留在 [`third-party/upstream-zcode/NOTICE.md`](third-party/upstream-zcode/NOTICE.md)，按 Apache License 2.0 第 4(d) 条随本作品分发。其中引用的源码路径与命名为上游原貌，本仓库更名后的对应位置见下文「修改说明」。

## 修改说明

依据 Apache License 2.0 第 4(b) 条声明：本仓库中的文件可能已被 ESCode 贡献者修改。主要修改包括：

- 产品更名为 ESCode：界面文案、包名（`@zcode/*` → `@escode/*`）、目录（如 `apps/zcode-cli` → `apps/escode-cli`）、Rust crate 与二进制名、环境变量（`ZCODE_*` → `ESCODE_*`）、数据与配置目录（`~/.zcode` → `~/.escode`、`.zcode/` → `.escode/`）、插件清单目录（`.escode-plugin`）、协议名与深链 scheme（`escode://`）。
- 应用图标、安装器背景与应用内字标替换为 ESCode 标识。
- Rust runtime 与 Node runtime 的行为对齐及其它功能改动，详见 `docs/specs/`。

为保持与上游在线服务的兼容，部分外部服务地址、接口路径、请求头名与 OAuth 回调地址保留原值，清单见 [`docs/specs/escode-rebrand.md`](docs/specs/escode-rebrand.md)。

## 商标

Apache License 2.0 不授予商标权（第 6 条）。「ZCode」名称与标识归其权利人所有；本作品不使用其作为产品名或标识，上文仅为说明来源而提及。

## 第三方组件

第三方组件的许可与声明见 [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) 与 [`third-party/`](third-party/)。
