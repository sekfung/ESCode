#!/usr/bin/env bash
# 为 macOS 上的 SSH P0 E2E 启动本机 sshd 目标：专用用户 + 密码登录 + 独立端口。
#
# 与 Linux 版 start-ssh-e2e-server.sh 保持同一契约：<port> <password> [workspace]，
# 调用方仍然只设置 ESCODE_E2E_SSH_*，不接触仓库 secrets。区别是 macOS runner 没有
# Docker，所以目标就是 runner 自身：sshd 监听 127.0.0.1，远端平台为 darwin-<arch>，
# 调用方必须用 ESCODE_REMOTE_ASSET_PLATFORMS=darwin-arm64（或 darwin-x64）准备 release。
set -euo pipefail

port="${1:?usage: start-ssh-e2e-server-macos.sh <port> <password> [workspace]}"
password="${2:?password required}"
user="${ESCODE_E2E_SSH_MACOS_USER:-escode-e2e}"
home_dir="/Users/${user}"
workspace="${3:-${home_dir}/workspace}"
config="/tmp/escode-e2e-sshd.conf"
log="/tmp/escode-e2e-sshd.log"
pid_file="/tmp/escode-e2e-sshd.pid"

if ! id -u "$user" >/dev/null 2>&1; then
  sudo sysadminctl -addUser "$user" -fullName "E2E SSH" -password "$password" -home "$home_dir" -shell /bin/zsh
else
  # 复用镜像里可能已存在的同名用户时，把密码重置成本次 run 的随机值。
  sudo sysadminctl -resetPasswordFor "$user" -newPassword "$password" >/dev/null 2>&1 ||
    sudo dscl . -passwd "/Users/$user" "$password"
fi

sudo mkdir -p "$home_dir" "$workspace"
sudo chown -R "$user" "$home_dir"

# macOS 10.13+ 在启用「远程登录」后会建立 com.apple.access_ssh 组，只允许组成员登录。
if dscl . -read /Groups/com.apple.access_ssh >/dev/null 2>&1; then
  sudo dseditgroup -o edit -a "$user" -t user com.apple.access_ssh
fi

# sshd 需要 host key；runner 镜像缺 key 时补生成，避免启动即退出。
sudo ssh-keygen -A >/dev/null

cat >"$config" <<EOF
Port ${port}
ListenAddress 127.0.0.1
PasswordAuthentication yes
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
UsePAM yes
PermitRootLogin no
AllowUsers ${user}
PidFile ${pid_file}
EOF

sudo pkill -f "sshd -f ${config}" >/dev/null 2>&1 || true
sudo /usr/sbin/sshd -f "$config" -E "$log"

# macOS 没有 coreutils timeout；用 nc -w 读 banner，超时由循环上限控制。
deadline=$((SECONDS + 180))
until banner="$(nc -w 2 127.0.0.1 "$port" </dev/null 2>/dev/null | head -c 4)" && [[ "$banner" == "SSH-" ]]; do
  if (( SECONDS > deadline )); then
    sudo cat "$log" >&2 || true
    echo "SSH target did not answer a handshake on 127.0.0.1:${port}" >&2
    exit 1
  fi
  sleep 2
done

echo "SSH target ready: 127.0.0.1:${port} workspace=${workspace} user=${user}"
