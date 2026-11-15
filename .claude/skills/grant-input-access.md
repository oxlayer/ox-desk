# Grant access to the input devices (LAN KVM)

The KVM reads `/dev/input/event*` and calls `EVIOCGRAB`. By default
these nodes belong to the `input` group, which the desktop user is not
part of.

## Run (once, needs sudo)

```bash
sudo install -m644 ~/dev/oxdesk/oxdesk/udev/70-oxdesk-input.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input
```

## Check

```bash
test -r /dev/input/event17 && echo ok || echo "no access"
getfacl -p /dev/input/event17 | grep '^user:'
# a line `user:<you>:rw-` must appear besides `user::rw-`
```

## Why `uaccess` and not the `input` group

The rule is `SUBSYSTEM=="input", ATTRS{idVendor}=="046d", TAG+="uaccess"`.

`uaccess` makes systemd-logind grant an ACL to the user of the **active
session**: access follows whoever is actually at the machine and goes
away when the session ends. Joining the `input` group would give
permanent access to **all** input devices, to any process of the
account, even with the session closed — a free passive keylogger.

A practical benefit: `uaccess` takes effect at `udevadm trigger` time; a
new group only takes effect in a new session (you have to log out).

The `046d` filter is Logitech, on purpose: the rule is not meant to open
the laptop keyboard or the webcam. To add a new device to the KVM, add its
`idVendor` explicitly.

## Find the devices

```bash
for d in /dev/input/event*; do
  n=$(cat /sys/class/input/$(basename $d)/device/name 2>/dev/null)
  case "$n" in *MX*|*Logitech*) echo "$d  $n";; esac
done
```
