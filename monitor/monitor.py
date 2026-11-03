#!/usr/bin/env python3
"""Painel de status das maquinas da LAN.

Stdlib apenas -- sem pip, sem Electron. Serve um HTML em localhost e um
/api/status em JSON. Adicionar uma terceira maquina = uma entrada em
hosts.json, nada de codigo.

  python3 monitor.py            # usa hosts.json ao lado
  python3 monitor.py outro.json
"""
from __future__ import annotations

import json
import socket
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent
CFG_PATH = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "hosts.json"
TIMEOUT = 1.5

_cache: dict = {"ts": 0.0, "data": None}
_lock = threading.Lock()


def load_cfg() -> dict:
    return json.loads(CFG_PATH.read_text(encoding="utf-8"))


def probe_port(addr: str, port: int) -> tuple[bool, float | None]:
    """TCP connect. Devolve (aberta, latencia_ms)."""
    t0 = time.perf_counter()
    try:
        with socket.create_connection((addr, port), timeout=TIMEOUT):
            return True, (time.perf_counter() - t0) * 1000
    except OSError:
        return False, None


def unit_state(unit: str) -> str:
    try:
        r = subprocess.run(
            ["systemctl", "--user", "is-active", unit],
            capture_output=True, text=True, timeout=5,
        )
        return r.stdout.strip() or "unknown"
    except Exception:
        return "unknown"


def ssh_detail(alias: str) -> dict:
    """Detalhe opcional via SSH. Silencioso em qualquer falha."""
    ps = (
        "$o=Get-CimInstance Win32_OperatingSystem;"
        "$c=Get-CimInstance Win32_ComputerSystem;"
        "$up=[int]((Get-Date)-$o.LastBootUpTime).TotalMinutes;"
        "$free=[math]::Round($o.FreePhysicalMemory/1MB,1);"
        "$tot=[math]::Round($c.TotalPhysicalMemory/1GB,1);"
        "\"$up|$free|$tot\""
    )
    try:
        r = subprocess.run(
            ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=3",
             "-o", "LogLevel=ERROR", alias, ps],
            capture_output=True, text=True, timeout=12,
        )
        line = [l for l in r.stdout.splitlines() if "|" in l]
        if not line:
            return {}
        up, free, tot = line[-1].strip().split("|")
        return {
            "uptime_min": int(float(up)),
            "ram_free_gb": float(free),
            "ram_total_gb": float(tot),
        }
    except Exception:
        return {}


def check_host(h: dict) -> dict:
    addr = h["addr"]
    svcs = []
    with ThreadPoolExecutor(max_workers=8) as ex:
        futs = {ex.submit(probe_port, addr, s["port"]): s for s in h.get("services", [])}
        for fut, s in futs.items():
            up, ms = fut.result()
            expect = s.get("expect", "up")
            svcs.append({
                "label": s["label"],
                "port": s["port"],
                "up": up,
                "ms": round(ms, 1) if ms else None,
                # "ok" = esta como deveria. Um servidor Deskflow local
                # fechado nao e problema: aqui a maquina e cliente.
                # "ok" = esta como deveria para ESTE papel.
                "ok": True if expect == "any" else ((not up) if expect == "down" else up),
                "expect": expect,
            })
    svcs.sort(key=lambda s: s["port"])
    online = any(s["up"] for s in svcs)
    out = {
        "name": h["name"],
        "addr": addr,
        "role": h.get("role", ""),
        "online": online,
        "services": svcs,
        "units": [{"unit": u, "state": unit_state(u)} for u in h.get("units", [])],
        "detail": {},
    }
    if online and h.get("ssh"):
        out["detail"] = ssh_detail(h["ssh"])
    return out


def snapshot(cfg: dict) -> dict:
    with ThreadPoolExecutor(max_workers=6) as ex:
        hosts = list(ex.map(check_host, cfg["hosts"]))
    return {"generated": time.strftime("%H:%M:%S"), "hosts": hosts}


def cached(cfg: dict) -> dict:
    ttl = max(2, int(cfg.get("refresh_seconds", 10)) - 2)
    with _lock:
        if _cache["data"] and time.time() - _cache["ts"] < ttl:
            return _cache["data"]
    data = snapshot(cfg)
    with _lock:
        _cache.update(ts=time.time(), data=data)
    return data


PAGE = """<!doctype html>
<html lang="pt-BR"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Maquinas</title>
<style>
  :root{
    color-scheme:light dark;
    --bg:#f4f5f7; --card:#fff; --fg:#111317; --dim:#666d78; --faint:#99a0ab;
    --line:#e5e8ec; --inset:#f8f9fb;
    --up:#0f7a3d; --down:#c02626; --warn:#8a6100;
    --upbg:#e6f6ec; --downbg:#fdeaea;
    --shadow:0 1px 2px rgba(16,20,28,.05), 0 8px 24px rgba(16,20,28,.05);
    --mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace;
  }
  @media (prefers-color-scheme:dark){:root{
    --bg:#0c0e12; --card:#15181e; --fg:#e9ebee; --dim:#949aa4; --faint:#5f6670;
    --line:#242932; --inset:#1b1f26;
    --up:#45d483; --down:#ff6b6b; --warn:#f0b429;
    --upbg:#0c2c1a; --downbg:#2d1113;
    --shadow:0 1px 2px rgba(0,0,0,.4), 0 10px 30px rgba(0,0,0,.3);
  }}
  *{box-sizing:border-box}
  html,body{height:100%}
  body{margin:0;background:var(--bg);color:var(--fg);
    font:14px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;
    -webkit-font-smoothing:antialiased;
    padding:env(safe-area-inset-top,0) 0 env(safe-area-inset-bottom,0)}
  .wrap{max-width:940px;margin:0 auto;padding:28px 20px 56px}

  header{display:flex;align-items:center;gap:14px;margin-bottom:6px}
  h1{font-size:19px;font-weight:660;margin:0;letter-spacing:-.015em}
  .summary{font-size:13px;color:var(--dim)}
  .summary b{color:var(--fg);font-weight:620}
  .spacer{flex:1}
  .stamp{font:11px/1 var(--mono);color:var(--faint);
    padding:5px 9px;border:1px solid var(--line);border-radius:7px;
    background:var(--card)}
  .sub{color:var(--faint);font-size:12px;margin:0 0 22px}

  .grid{display:grid;gap:16px;grid-template-columns:repeat(auto-fit,minmax(310px,1fr))}
  .card{background:var(--card);border:1px solid var(--line);border-radius:14px;
    padding:18px;box-shadow:var(--shadow);
    display:flex;flex-direction:column;gap:14px}

  .top{display:flex;align-items:flex-start;gap:12px}
  .dot{width:9px;height:9px;border-radius:50%;flex:none;margin-top:6px;
    background:var(--down);box-shadow:0 0 0 3px var(--downbg)}
  .dot.on{background:var(--up);box-shadow:0 0 0 3px var(--upbg)}
  .dot.on::after{content:"";display:block;width:9px;height:9px;border-radius:50%;
    background:var(--up);animation:pulse 2.4s ease-out infinite}
  @keyframes pulse{0%{transform:scale(1);opacity:.5}
                   70%{transform:scale(2.6);opacity:0}100%{opacity:0}}
  @media (prefers-reduced-motion:reduce){.dot.on::after{animation:none}}
  .ident{min-width:0}
  .nm{font-weight:640;font-size:15px;letter-spacing:-.01em}
  .ro{color:var(--dim);font-size:12px;margin-top:1px}
  .ro span{font-family:var(--mono);font-size:11px}

  .svc{background:var(--inset);border-radius:9px;padding:2px 11px}
  table{width:100%;border-collapse:collapse}
  td{padding:7px 0;border-bottom:1px solid var(--line);vertical-align:baseline}
  tr:last-child td{border-bottom:0}
  td.l{color:var(--fg);font-size:13px}
  td.p{color:var(--faint);font:11px var(--mono);text-align:right;
    width:4.8em;padding-right:10px}
  td.s{text-align:right;width:6.2em;font:12px var(--mono);font-weight:600;
    color:var(--dim)}
  .ok{color:var(--up)} .no{color:var(--down)} .wa{color:var(--warn)}

  .units{display:flex;flex-wrap:wrap;gap:6px}
  .u{font:11px var(--mono);padding:3px 8px;border-radius:6px;
    background:var(--inset);border:1px solid var(--line);color:var(--faint)}
  .u.active{color:var(--up);border-color:color-mix(in srgb,var(--up) 25%,var(--line))}
  .u.bad{color:var(--down);border-color:color-mix(in srgb,var(--down) 25%,var(--line))}

  .det{display:flex;flex-wrap:wrap;gap:4px 16px;color:var(--dim);font-size:12px;
    padding-top:2px;border-top:1px solid var(--line)}
  .det b{font-family:var(--mono);font-weight:600;color:var(--fg);font-size:11.5px}

  .bar{height:3px;border-radius:2px;background:var(--line);overflow:hidden;
    margin-top:3px;width:100%}
  .bar i{display:block;height:100%;background:var(--up);border-radius:2px;
    transition:width .5s ease}

  footer{margin-top:26px;color:var(--faint);font-size:12px;
    display:flex;flex-wrap:wrap;gap:4px 14px}
  code{font:11px var(--mono);background:var(--card);padding:2px 6px;
    border-radius:5px;border:1px solid var(--line)}
  .err{background:var(--downbg);color:var(--down);padding:12px 14px;
    border-radius:10px;font-size:13px}
</style></head><body><div class="wrap">
<header>
  <h1>Maquinas</h1>
  <div class="summary" id="sum"></div>
  <div class="spacer"></div>
  <div class="stamp" id="stamp">...</div>
</header>
<p class="sub">LAN 192.168.1.0/24 &middot; leitura direta de portas, sem agente remoto</p>
<div class="grid" id="grid"></div>
<footer>
  <span>atualiza a cada <b id="rs"></b>s</span>
  <span>JSON em <code>/api/status</code></span>
  <span>hosts em <code>hosts.json</code></span>
</footer>
</div>
<script>
const RS = __REFRESH__;
document.getElementById('rs').textContent = RS;
const esc = t => String(t).replace(/[&<>"]/g, c =>
  ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));

function svcRow(s){
  let cls = '', txt;
  if (s.expect === 'down')     { cls = s.up ? 'wa' : 'ok'; txt = s.up ? 'aberta' : 'fechada'; }
  else if (s.expect === 'any') { txt = s.up ? s.ms + ' ms' : 'off'; }
  else { cls = s.up ? 'ok' : 'no';
         txt = s.up ? (s.ms != null ? s.ms + ' ms' : 'ok') : 'fora'; }
  return `<tr><td class="l">${esc(s.label)}</td>
          <td class="p">:${s.port}</td>
          <td class="s ${cls}">${txt}</td></tr>`;
}

function card(h){
  const units = h.units.map(u => {
    const c = u.state === 'active' ? 'active' : (u.state === 'unknown' ? '' : 'bad');
    return `<span class="u ${c}">${esc(u.unit.replace('.service',''))} &middot; ${esc(u.state)}</span>`;
  }).join('');

  const d = h.detail, det = [];
  if (d.uptime_min != null){
    const m = d.uptime_min, dd = Math.floor(m/1440), hh = Math.floor((m%1440)/60);
    det.push(`<span>no ar <b>${dd ? dd+'d ' : ''}${hh}h</b></span>`);
  }
  if (d.ram_total_gb){
    const used = d.ram_total_gb - d.ram_free_gb;
    const pct = Math.round(used / d.ram_total_gb * 100);
    det.push(`<span>RAM <b>${used.toFixed(1)}/${d.ram_total_gb} GB</b>
              <span class="bar"><i style="width:${pct}%"></i></span></span>`);
  }

  return `<div class="card">
    <div class="top">
      <span class="dot ${h.online ? 'on' : ''}"></span>
      <div class="ident">
        <div class="nm">${esc(h.name)}</div>
        <div class="ro">${esc(h.role)} &middot; <span>${esc(h.addr)}</span></div>
      </div>
    </div>
    <div class="svc"><table>${h.services.map(svcRow).join('')}</table></div>
    ${units ? `<div class="units">${units}</div>` : ''}
    ${det.length ? `<div class="det">${det.join('')}</div>` : ''}
  </div>`;
}

async function tick(){
  const grid = document.getElementById('grid');
  try{
    const r = await fetch('/api/status', {cache:'no-store'});
    if (!r.ok) throw new Error('HTTP ' + r.status);
    const d = await r.json();
    grid.innerHTML = d.hosts.map(card).join('');
    const on = d.hosts.filter(h => h.online).length;
    const bad = d.hosts.reduce((n,h) => n + h.services.filter(s => !s.ok).length, 0);
    document.getElementById('sum').innerHTML = bad
      ? `<b>${on}/${d.hosts.length}</b> online &middot; <b>${bad}</b> servico(s) com problema`
      : `<b>${on}/${d.hosts.length}</b> online &middot; tudo certo`;
    document.getElementById('stamp').textContent = d.generated;
  }catch(e){
    grid.innerHTML = `<div class="err">painel inacessivel: ${esc(e.message)}</div>`;
    document.getElementById('stamp').textContent = '--:--:--';
  }
}
tick(); setInterval(tick, RS*1000);
</script></body></html>"""


class Handler(BaseHTTPRequestHandler):
    cfg: dict = {}

    def log_message(self, *a):  # silencia o access log
        pass

    def _send(self, code, body: bytes, ctype):
        try:
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            # Cliente desistiu no meio (health check com timeout curto,
            # aba fechada). Nao e erro nosso -- nao sujar o journal.
            pass

    def do_GET(self):
        if self.path.startswith("/healthz"):
            # De proposito nao toca em cached(): o /api/status faz probe de
            # rede e SSH e pode levar segundos. Health check tem que ser barato.
            self._send(200, b'{"ok":true}', "application/json")
        elif self.path.startswith("/api/status"):
            body = json.dumps(cached(self.cfg), ensure_ascii=False).encode()
            self._send(200, body, "application/json; charset=utf-8")
        elif self.path in ("/", "/index.html"):
            rs = int(self.cfg.get("refresh_seconds", 10))
            html = PAGE.replace("__REFRESH__", str(rs))
            self._send(200, html.encode(), "text/html; charset=utf-8")
        else:
            self._send(404, b"nao encontrado", "text/plain; charset=utf-8")


def main():
    cfg = load_cfg()
    Handler.cfg = cfg
    port = int(cfg.get("port", 8787))
    # 127.0.0.1 de proposito: o painel le portas e roda SSH, nao deve
    # ficar exposto na LAN. Para abrir pra rede, mude aqui ciente disso.
    srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    print(f"painel em http://127.0.0.1:{port}  (config: {CFG_PATH})", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
