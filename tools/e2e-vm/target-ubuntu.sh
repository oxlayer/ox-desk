#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# The Ubuntu TARGET VM for the end-to-end tests (specs/026): GNOME on Wayland
# with automatic login, the machine OxDesk sends keyboard and mouse TO.
#
#   tools/e2e-vm/target-ubuntu.sh build    # download, configure, first boot (once)
#   tools/e2e-vm/target-ubuntu.sh start    # boot it (after build)
#   tools/e2e-vm/target-ubuntu.sh stop
#   tools/e2e-vm/target-ubuntu.sh ssh [cmd...]
#   tools/e2e-vm/target-ubuntu.sh shot <file.ppm>   # a screenshot, via QMP
#
# No root needed: QEMU with KVM (the user must be able to open /dev/kvm),
# user-mode networking for the internet and SSH (host port $SSH_PORT), and a
# private L2 shared with the source VM over a multicast socket (10.77.0.0/24).
# cloud-init reads its seed from a tiny HTTP server on the host through the
# SMBIOS serial, so no seed ISO and no cloud-image-utils are needed.
#
# The display is VNC on 127.0.0.1 (port 5900 + $VNC_DISPLAY), not `none`:
# with no display the guest sees no connected monitor and GNOME lights no
# output ("Display output is not active" in the first screenshot).
#
# Everything lives in $E2E_DIR (default ~/.cache/oxdesk-e2e), never in the
# repository: the VM's disk, its SSH key, its QMP socket, its log.
set -uo pipefail

E2E_DIR=${E2E_DIR:-$HOME/.cache/oxdesk-e2e}
NAME=target-ubuntu
SERIES=${SERIES:-resolute}
IMAGE_URL="https://cloud-images.ubuntu.com/$SERIES/current/$SERIES-server-cloudimg-amd64.img"
SSH_PORT=${SSH_PORT:-2222}
SEED_PORT=${SEED_PORT:-8765}
PRIVATE_IP=10.77.0.2
MCAST=230.77.0.1:7701
D="$E2E_DIR/$NAME"
fail() { echo "FAIL: $*" >&2; exit 1; }

mkdir -p "$D" || fail "cannot create $D"
KEY="$E2E_DIR/id_ed25519"
[ -f "$KEY" ] || ssh-keygen -q -t ed25519 -N "" -C "oxdesk-e2e" -f "$KEY" || fail "ssh-keygen"

qemu_up() { [ -f "$D/pid" ] && kill -0 "$(cat "$D/pid")" 2>/dev/null; }

vm_ssh() {
  ssh -i "$KEY" -p "$SSH_PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
      -o LogLevel=ERROR -o ConnectTimeout=5 -o BatchMode=yes tester@127.0.0.1 "$@"
}

seed() {
  # The cloud-init seed, served on 127.0.0.1 (the guest reaches it as 10.0.2.2).
  mkdir -p "$D/seed"
  printf 'instance-id: %s\nlocal-hostname: e2e-target\n' "$NAME-$(date +%s)" > "$D/seed/meta-data"
  : > "$D/seed/vendor-data"
  cat > "$D/seed/user-data" <<EOF
#cloud-config
hostname: e2e-target
users:
  - name: tester
    shell: /bin/bash
    groups: [sudo, input, video]
    sudo: "ALL=(ALL) NOPASSWD:ALL"
    lock_passwd: true
    ssh_authorized_keys:
      - $(cat "$KEY.pub")
package_update: true
packages:
  - ubuntu-desktop-minimal
  - gdm3
write_files:
  - path: /etc/gdm3/custom.conf
    content: |
      [daemon]
      AutomaticLoginEnable=true
      AutomaticLogin=tester
      WaylandEnable=true
  - path: /etc/netplan/60-e2e-private.yaml
    permissions: "0600"
    content: |
      network:
        version: 2
        ethernets:
          private:
            match: {macaddress: "52:54:00:77:00:02"}
            addresses: [$PRIVATE_IP/24]
runcmd:
  - [netplan, apply]
  - [systemctl, set-default, graphical.target]
power_state:
  mode: reboot
  condition: true
EOF
}

boot() {
  qemu_up && { echo "already running (pid $(cat "$D/pid"))"; return 0; }
  qemu-system-x86_64 \
    -name "$NAME" -machine q35,accel=kvm -cpu host -smp 4 -m 6144 \
    -drive "file=$D/disk.qcow2,if=virtio,format=qcow2" \
    -device virtio-vga -display "vnc=127.0.0.1:${VNC_DISPLAY:-5}" \
    -smbios "type=1,serial=ds=nocloud;s=http://10.0.2.2:$SEED_PORT/" \
    -netdev "user,id=net0,hostfwd=tcp:127.0.0.1:$SSH_PORT-:22" -device virtio-net-pci,netdev=net0 \
    -netdev "socket,id=net1,mcast=$MCAST" -device virtio-net-pci,netdev=net1,mac=52:54:00:77:00:02 \
    -qmp "unix:$D/qmp.sock,server,nowait" \
    -serial "file:$D/serial.log" \
    -pidfile "$D/pid" -daemonize || fail "qemu did not start"
  echo "booted (pid $(cat "$D/pid")); serial log: $D/serial.log"
}

wait_ssh() {
  local limit=$1
  for _ in $(seq 1 "$limit"); do vm_ssh true 2>/dev/null && return 0; sleep 10; done
  return 1
}

case "${1:-}" in
  build)
    qemu_up && fail "it is running; stop it first"
    [ -f "$D/base.img" ] || { echo "downloading $IMAGE_URL"; curl -fL --retry 3 -o "$D/base.img.part" "$IMAGE_URL" && mv "$D/base.img.part" "$D/base.img"; } || fail "download"
    rm -f "$D/disk.qcow2"
    qemu-img create -q -f qcow2 -F qcow2 -b "$D/base.img" "$D/disk.qcow2" 30G || fail "disk"
    seed
    ( cd "$D/seed" && exec python3 -m http.server "$SEED_PORT" --bind 127.0.0.1 ) > "$D/seed.log" 2>&1 &
    SEED_PID=$!
    trap 'kill $SEED_PID 2>/dev/null' EXIT
    boot
    echo "installing the desktop (cloud-init; minutes, then a reboot)..."
    # First boot: SSH comes up early, cloud-init then installs the desktop
    # and reboots. Wait for the reboot to have happened: cloud-init done.
    wait_ssh 90 || fail "no ssh after 15 min (see $D/serial.log)"
    vm_ssh 'cloud-init status --wait' >/dev/null 2>&1
    # cloud-init's reboot comes right after it reports done; without the
    # pause, the next check can still reach the old sshd and call it built.
    sleep 45
    wait_ssh 60 || fail "no ssh after the reboot"
    echo "built: $(vm_ssh 'lsb_release -ds; systemctl get-default')"
    ;;
  start)
    [ -f "$D/disk.qcow2" ] || fail "not built: run build first"
    boot
    wait_ssh 30 || fail "no ssh"
    echo "up: $(vm_ssh 'hostname')"
    ;;
  stop)
    if qemu_up; then
      vm_ssh 'sudo poweroff' 2>/dev/null
      for _ in $(seq 1 30); do qemu_up || break; sleep 1; done
      qemu_up && kill "$(cat "$D/pid")"
    fi
    rm -f "$D/pid"; echo "stopped"
    ;;
  ssh) shift; vm_ssh "$@" ;;
  shot)
    out=${2:?usage: shot <file.ppm>}
    printf '{"execute":"qmp_capabilities"}\n{"execute":"screendump","arguments":{"filename":"%s"}}\n' "$(realpath -m "$out")" \
      | python3 -c "import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.sendall(sys.stdin.buffer.read()); import time; time.sleep(1); print(s.recv(65536).decode(errors='replace')[-200:])" "$D/qmp.sock"
    ;;
  *) sed -n '5,13p' "$0"; exit 2 ;;
esac
