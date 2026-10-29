#!/usr/bin/env bash
# 为 SSH P0 E2E 启动独占的 Ubuntu 24.04 SSH 目标：容器内 sshd，root + password 登录。
#
# 用法：start-ssh-e2e-server.sh <port> <password> [workspace]
# 环境：
#   ESCODE_E2E_SSH_IMAGE      镜像（默认 ubuntu:24.04）
#   ESCODE_E2E_SSH_CONTAINER  容器名（默认 escode-e2e-ssh）
#   ESCODE_E2E_SSH_APT_MIRROR 可选 apt 镜像（本地受限网络调试用；CI 留空走默认源）
#   ESCODE_E2E_SSH_HOST_NETWORK=1 用 host 网络直接在目标端口监听（本地 DNS 受限时调试用）
#
# 目标只用于单次 E2E：容器销毁即恢复干净状态；密码只经容器环境变量传入，
# 不写入仓库、日志或测试产物（CI 调用方负责 ::add-mask::）。
set -euo pipefail

port="${1:?usage: start-ssh-e2e-server.sh <port> <password> [workspace]}"
password="${2:?password required}"
workspace="${3:-/root/workspace}"
image="${ESCODE_E2E_SSH_IMAGE:-ubuntu:24.04}"
name="${ESCODE_E2E_SSH_CONTAINER:-escode-e2e-ssh}"
apt_mirror="${ESCODE_E2E_SSH_APT_MIRROR:-}"
host_network="${ESCODE_E2E_SSH_HOST_NETWORK:-0}"

network_args=(-p "127.0.0.1:${port}:22")
sshd_port=""
if [[ "$host_network" == "1" ]]; then
  network_args=(--network host)
  sshd_port="$port"
fi

docker rm -f "$name" >/dev/null 2>&1 || true

docker run -d --name "$name" \
  "${network_args[@]}" \
  -e "E2E_SSH_PASSWORD=$password" \
  -e "E2E_SSH_WORKSPACE=$workspace" \
  -e "E2E_APT_MIRROR=$apt_mirror" \
  -e "E2E_SSH_PORT=$sshd_port" \
  "$image" \
  bash -c '
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive
    if [ -n "${E2E_APT_MIRROR:-}" ]; then
      sed -i \
        -e "s|http://archive.ubuntu.com/ubuntu|$E2E_APT_MIRROR|g" \
        -e "s|http://security.ubuntu.com/ubuntu|$E2E_APT_MIRROR|g" \
        /etc/apt/sources.list.d/ubuntu.sources
    fi
    apt-get update -qq
    apt-get install -y --no-install-recommends \
      openssh-server ca-certificates curl git gzip procps tar
    mkdir -p /run/sshd "$E2E_SSH_WORKSPACE"
    printf "root:%s\n" "$E2E_SSH_PASSWORD" | chpasswd
    printf "%s\n" \
      "PermitRootLogin yes" \
      "PasswordAuthentication yes" \
      "KbdInteractiveAuthentication no" \
      "UsePAM yes" \
      >> /etc/ssh/sshd_config
    if [ -n "${E2E_SSH_PORT:-}" ]; then
      printf "Port %s\n" "$E2E_SSH_PORT" >> /etc/ssh/sshd_config
    fi
    exec /usr/sbin/sshd -D -e
  ' >/dev/null

deadline=$((SECONDS + 180))
until banner="$(timeout 3 bash -c "exec 3<>/dev/tcp/127.0.0.1/${port}; head -c 4 <&3" 2>/dev/null)" \
  && [[ "$banner" == "SSH-" ]]; do
  if [[ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null || true)" != "true" ]]; then
    docker logs "$name" >&2 || true
    echo "SSH target container exited before becoming ready" >&2
    exit 1
  fi
  if (( SECONDS > deadline )); then
    docker logs "$name" >&2 || true
    echo "SSH target did not answer a handshake on 127.0.0.1:${port}" >&2
    exit 1
  fi
  sleep 2
done

echo "SSH target ready: 127.0.0.1:${port} workspace=${workspace} image=${image}"
