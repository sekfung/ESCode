#!/usr/bin/env bash
# 为 Windows 客户端的 SSH P0 E2E 在 WSL2 里启动 Linux SSH 目标。
#
# 必须在 WSL 发行版内以 root 运行：调用方（workflow）从 Windows 侧执行
#   wsl -d <distro> -u root -- bash <repo>/scripts/e2e/start-ssh-e2e-server-wsl.sh <password> [workspace]
# 与 Linux 容器版保持同一契约：root + 密码登录 + 独立端口，只连 127.0.0.1。
# 远程平台固定 linux-x64（WSL runner 是 x64），客户端需要用同一平台准备远端 release。
set -euo pipefail

password="${1:?password required}"
workspace="${2:-/root/workspace}"
port="${ESCODE_E2E_SSH_PORT:-2222}"
log="/tmp/escode-e2e-sshd.log"
config="/tmp/escode-e2e-sshd.conf"

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends \
  openssh-server ca-certificates curl git gzip procps tar

mkdir -p /run/sshd "$workspace"
printf "root:%s\n" "$password" | chpasswd

cat >"$config" <<EOF
Port ${port}
ListenAddress 0.0.0.0
PermitRootLogin yes
PasswordAuthentication yes
KbdInteractiveAuthentication no
UsePAM yes
PidFile /tmp/escode-e2e-sshd.pid
EOF

# WSL runner 镜像里通常没有 systemd；直接起 sshd，避免 service 脚本依赖 init。
pkill -f "sshd -f ${config}" >/dev/null 2>&1 || true
/usr/sbin/sshd -f "$config" -E "$log"

for _ in $(seq 1 60); do
  if exec 3<>"/dev/tcp/127.0.0.1/${port}" 2>/dev/null; then
    exec 3<&- 3>&-
    echo "SSH target ready: 127.0.0.1:${port} workspace=${workspace}"
    exit 0
  fi
  sleep 2
done

cat "$log" >&2 || true
echo "SSH target did not answer on 127.0.0.1:${port}" >&2
exit 1
