#!/usr/bin/env python3
"""Deteccao de borda via org.gnome.Mutter.InputCapture (direto, sem portal).

O portal InputCapture do GNOME aceita CreateSession e nunca responde --
43 requests ficaram pendentes no backend. O mutter expoe a MESMA
funcionalidade sincronamente, sem dialogo. Este e o caminho.
"""
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
D = "org.gnome.Mutter.InputCapture"
SESSION_IFACE = f"{D}.Session"
loop = GLib.MainLoop()
hits = [0]


def call(path, iface, method, args, ret):
    return bus.call_sync(D, path, iface, method, args,
                         GLib.VariantType(ret) if ret else None,
                         Gio.DBusCallFlags.NONE, 10000, None)


sess = call("/org/gnome/Mutter/InputCapture", D, "CreateSession",
            GLib.Variant("(u)", (2,)), "(o)").unpack()[0]
print(f"sessao: {sess}", flush=True)

serial, zones = call(sess, SESSION_IFACE, "GetZones", None, "(ua(uuii))").unpack()
print(f"serial={serial} zones={zones}", flush=True)

w, h, x, y = zones[0]
x2 = x + w   # FRONTEIRA da zona. x+w-1 fica DENTRO e o mutter recusa:
             # "Line overlaps with monitor region"
bid = call(sess, SESSION_IFACE, "AddBarrier",
           GLib.Variant("(u(iiii))", (serial, (x2, y, x2, y + h))), "(u)").unpack()[0]
print(f"barreira id={bid} em x={x2}, y={y}..{y+h}  (borda DIREITA)", flush=True)


def on_activated(conn, sender, path, iface, signal, params):
    barrier_id, activation_id, pos = params.unpack()
    print(f"\n*** ACTIVATED *** barreira={barrier_id} activation={activation_id} cursor={pos}", flush=True)
    try:
        call(sess, SESSION_IFACE, "Release", GLib.Variant("(a{sv})", ({},)), None)
        print("[Release] ponteiro devolvido", flush=True)
    except GLib.Error as e:
        print(f"[Release] erro: {e.message}", flush=True)
    hits[0] += 1
    if hits[0] >= 2:
        print("\n==> FUNCIONA: evento de borda + devolucao do ponteiro, sem libei.", flush=True)
        try:
            call(sess, SESSION_IFACE, "Close", None, None)
        except GLib.Error:
            pass
        loop.quit()


for sig in ("Activated", "Deactivated", "Disabled", "Closed", "ZonesChanged"):
    bus.signal_subscribe(D, SESSION_IFACE, sig, sess, None, Gio.DBusSignalFlags.NONE,
                         on_activated if sig == "Activated"
                         else (lambda c, s, p, i, g, prm, _n=sig: print(f"[{_n}] {prm.unpack()}", flush=True)))

# Hipotese a eliminar: o mutter talvez so arme as barreiras depois de
# alguem conectar o canal de input (EIS). Conectamos e seguramos o fd.
try:
    reply, fds = bus.call_with_unix_fd_list_sync(
        D, sess, SESSION_IFACE, "ConnectToEIS", None, GLib.VariantType("(h)"),
        Gio.DBusCallFlags.NONE, 10000, None, None)
    idx = reply.unpack()[0]
    fd = fds.get(idx) if fds else None
    print(f"[ConnectToEIS] ok, fd={fd}", flush=True)
except GLib.Error as e:
    print(f"[ConnectToEIS] erro (seguindo sem ele): {e.message}", flush=True)

call(sess, SESSION_IFACE, "Enable", None, None)
print("[Enable] ok -- MOVA O PONTEIRO PARA A BORDA DIREITA DA TELA (2x)", flush=True)

GLib.timeout_add_seconds(300, lambda: (print("\n[timeout 300s sem evento]", flush=True), loop.quit())[1])
loop.run()
