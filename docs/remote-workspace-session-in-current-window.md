# Remote Workspace Session in a Workspace Window（当前事实）

远程 workspace 作为当前 workspace window 内的逻辑 session/tab 打开，不为每个 workspace 另建 BrowserWindow；
本地与远端 scope 统一由 App 级 Rust Host 承载（S6 起 Node 窗口 Host 已删除），窗口只持有 attachment。SSH 按 `remoteHostKey` 共享 Host 内连接，WSL 按
`normalized(distro,user)` 共享 Host 内连接；Docker/Server 使用 dedicated connection，但不创建额外 Host PID。

```text
Workspace Windows (attachments)
  └─ App 级 Rust Host（唯一 PID，多窗口共享）
      ├─ LocalSource -> local workspaceKey -> local escode-cli
      └─ RemoteConnectionRegistry
          ├─ shared SSH connection (remoteHostKey)
          │  ├─ remoteSessionId A -> ssh workspaceKey A
          │  └─ remoteSessionId B -> ssh workspaceKey B
          ├─ shared WSL connection (normalized distro/user)
          │  ├─ remoteSessionId C -> wsl workspaceKey C
          │  └─ remoteSessionId D -> wsl workspaceKey D
          ├─ dedicated Docker connection -> docker workspaceKey
          └─ dedicated Server connection -> server workspaceKey
```

## 身份与路由

- `workspaceIdentity` 表示隔离身份，`workspacePath` 表示目标环境中的真实路径。
- session、tab、CLI、缓存、队列、历史和远控 key 使用
  `workspaceKey = workspaceIdentity?.trim() || workspacePath`。
- 文件操作与命令 cwd 使用 `workspacePath`。
- 远程 workspace 必须传递统一构造的 `workspaceIdentity`；远控还必须传递 `remoteSessionId`。
- renderer 通过 workspace service resolver 选择 workspace scope，不能只按路径匹配。

## 生命周期

```text
select/connect remote workspace
  -> establish remote workspace scope
  -> register workspaceIdentity + remoteSessionId
  -> Main forwards request to the App 级 Rust Host
  -> Rust Host allocates remoteSessionId and resolves/creates connection
  -> Rust Host creates identity-validated scoped attachment
  -> pane subscribes session projection
  -> mobile may attach to the same Host/CLI
```

关闭 pane 或切 workspace 不应误杀其他 workspace 的 CLI；关闭单个 attachment 不销毁 Rust Host 或仍被使用的连接。
WSL logical session 关闭时按 workspace generation 释放对应 runtime，最后 owner 消失后保留 60 秒 idle TTL；
Docker/Server dedicated connection 按 logical session 生命周期回收。Main/relay 只做连接请求、端口与 attachment
调度，不复制远端 session 状态。

## 回归边界

- 同路径但不同 remote identity 必须隔离。
- desktop continuous 与 mobile replayable 必须连接同一 workspace CLI。
- 切换 workspace 后，文件/terminal cwd 使用新 `workspacePath`，缓存与 session 按 workspaceKey 隔离。
- 远程 workspace 不得另起独立 Host 或 Agent runtime；它只能 attach App 级 Rust Host 的既有 scope。
- 多窗口共享同一 App 级 Rust Host，身份、端口与故障域由 Host 内的 scope 隔离，不靠每窗口一个进程。
