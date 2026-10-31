import type { RemoteAssetInstallMode } from "./remoteAssetInstallMode.js";
import type { RemoteResourcePackageSelection } from "./remoteResourcePackages.js";

/**
 * Connection Profile 描述这条远端连接时的两个标识（S3-c：`source-server-remote-rust-host.md` §10.12）。
 * 携带 `profileId` 表示连接事实由 Rust Host 按 profile 建立；其余字段只是脱敏快照。
 */
export interface ProfileScopedRemoteTarget {
  sourceServerId?: string;
  profileId?: string;
}

export interface SSHConnectOptions extends ProfileScopedRemoteTarget {
  kind: "ssh";
  host: string;
  port?: number;
  username: string;
  sshConfigAlias?: string;
  password?: string;
  privateKeyPath?: string;
  privateKeyPassphrase?: string;
  assetInstallMode?: RemoteAssetInstallMode;
  resourcePackages?: RemoteResourcePackageSelection;
}

export interface WSLConnectOptions extends ProfileScopedRemoteTarget {
  kind: "wsl";
  distro?: string;
  user?: string;
}

/**
 * WSL 目标经 canonical 解析后的形态（`distro` / `user` 必有）。
 *
 * 直接写 `Required<WSLConnectOptions>` 会把后加的可选字段（`sourceServerId`/`profileId`）
 * 也算成必填，失去“只要求解析结果字段”的语义。
 */
export type ResolvedWSLConnectOptions = WSLConnectOptions & { distro: string; user: string };

export interface DockerConnectOptions extends ProfileScopedRemoteTarget {
  kind: "docker";
  container: string;
}

export interface ServerConnectOptions extends ProfileScopedRemoteTarget {
  kind: "server";
  /** 已运行的 escode-server HTTP(S)/WS(S) 入口。 */
  url: string;
  /** 用户可读名称，仅用于展示。 */
  name?: string;
  /** 临时连接凭据；持久化时必须转成 credential key。 */
  token?: string;
  /** 服务端默认工作区路径，可被连接流程中的 workspacePath 覆盖。 */
  workspacePath?: string;
  /** 远端服务实例稳定标识，用于 workspaceIdentity authority。 */
  serverId?: string;
}

export type RemoteTarget =
  | SSHConnectOptions
  | WSLConnectOptions
  | DockerConnectOptions
  | ServerConnectOptions;

/** 删除只应存在于当前连接流程中的 secret，供长期内存状态和跨进程回包使用。 */
export function stripRemoteTargetSecrets(target: RemoteTarget): RemoteTarget {
  if (target.kind === "ssh") {
    const {
      password: _password,
      privateKeyPassphrase: _privateKeyPassphrase,
      ...sanitized
    } = target;
    return sanitized;
  }

  if (target.kind === "server") {
    const { token: _token, ...sanitized } = target;
    return sanitized;
  }

  return target;
}
