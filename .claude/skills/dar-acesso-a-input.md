# Dar acesso aos dispositivos de input (KVM por LAN)

O KVM lê `/dev/input/event*` e chama `EVIOCGRAB`. Por padrão esses nós
pertencem ao grupo `input`, do qual o usuário de desktop não faz parte.

## Rodar (uma vez, precisa de sudo)

```bash
sudo install -m644 ~/dev/logi/logid/udev/70-logid-input.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input
```

## Conferir

```bash
test -r /dev/input/event17 && echo ok || echo "sem acesso"
getfacl -p /dev/input/event17 | grep '^user:'
# tem de aparecer uma linha `user:<voce>:rw-` além de `user::rw-`
```

## Por que `uaccess` e não o grupo `input`

A regra é `SUBSYSTEM=="input", ATTRS{idVendor}=="046d", TAG+="uaccess"`.

`uaccess` faz o systemd-logind dar ACL ao usuário da **sessão ativa**:
o acesso segue quem está de fato na máquina e some quando a sessão
acaba. Entrar no grupo `input` daria acesso permanente a **todos** os
dispositivos de entrada, para qualquer processo da conta, inclusive com
a sessão encerrada — um keylogger passivo de graça.

Bônus prático: `uaccess` vale na hora do `udevadm trigger`; grupo novo
só entra em vigor em sessão nova (precisa deslogar).

O filtro `046d` é a Logitech, de propósito: não é para abrir o teclado
do laptop nem a webcam. Dispositivo novo no KVM = acrescentar o
`idVendor` dele explicitamente.

## Achar os dispositivos

```bash
for d in /dev/input/event*; do
  n=$(cat /sys/class/input/$(basename $d)/device/name 2>/dev/null)
  case "$n" in *MX*|*Logitech*) echo "$d  $n";; esac
done
```
