# TMPDIR 写入收口规范

## 背景

系统临时目录可能被宿主环境、沙箱或企业安全策略限制权限。ESCode 产品主路径如果依赖 `TMPDIR` / `os.tmpdir()` 写入，会导致日志导出、反馈上传、剪贴板图片读取、checkpoint 构建或命令输出持久化在部分环境中失败。

## 目标

- 产品运行时不再把可控的 ESCode 临时/持久化文件写入系统 `TMPDIR`。
- 新路径统一落在 ESCode 数据根目录下，默认是 `~/.escode`。
- 不迁移、不读取、不兼容旧 `TMPDIR` 残留数据。
- 本次不修改 remote asset fallback，也不修改官方 iOS / Android 模拟器插件路径。

## 路径规范

| 场景                       | 新路径                           | 生命周期                         |
| -------------------------- | -------------------------------- | -------------------------------- |
| escode-cli Bash 持久化输出 | `~/.escode/cli/exec`             | 按 session/task 输出保留         |
| escode-cli 剪贴板图片      | `~/.escode/clipboard`            | 单次读取后删除                   |
| 日志导出 staging           | `~/.escode/export-log-stage`     | 打包完成后删除，异常退出允许残留 |
| 日志导出结果               | `~/.escode/export-log`           | 用户可见导出结果，默认保留       |
| feedback 附件和日志包      | `~/.escode/feedback`             | 上传完成后删除                   |
| Git checkpoint 临时 index  | `~/.escode/git-checkpoint-index` | checkpoint 构建完成后删除        |
| Desktop 编辑器图标转换     | `~/.escode/editor-icon`          | 单次转换后删除                   |

## 约束

- 写入前必须确保父目录存在，不能依赖 `mkdtemp` 自动创建父目录。
- 短生命周期目录必须使用单次唯一子目录，结束后在 `finally` 中清理。
- 失败清理不能影响主流程结果，但残留必须留在上述可控目录中，便于后续“完全清理”功能删除。
- escode-cli 主流程仍优先使用配置解析后的 `storageRoot`；只有 adapter 默认兜底才使用默认 `~/.escode/cli/exec`。
- service / desktop 侧使用现有 data base dir 语义，默认落到 `~/.escode`，配置了 `ESCODE_DATA_BASE_DIR` 时落到 `{dataBaseDir}/.escode`。

## 非目标

- 不清理已有 `$TMPDIR/escode-*` 残留。
- 不修改 remote asset cache fallback。
- 不修改 `apps/escode-cli/packages/ios-simulator-plugin` 与 `apps/escode-cli/packages/android-emulator-plugin` 的数据目录。
