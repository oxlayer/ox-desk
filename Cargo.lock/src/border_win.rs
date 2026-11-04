//! Vigilancia de borda no Windows: polling de `GetCursorPos`.
//!
//! Era o modo `watch-edge` do `logid-agent`, processo separado lancado por
//! atalho no `shell:startup`. Virou thread do daemon quando o `logid`
//! passou a rodar no Windows -- com os dois vivos, seriam dois donos do
//! mesmo receptor HID++, exatamente o que a trava de instancia unica
//! existe para impedir (ja aconteceu: tres `watch-edge` simultaneos
//! triplicando o trafego de radio).
//!
//! Por isso esta thread reivindica o **mesmo nome de mutex** do
//! `watch-edge`. Nao e coincidencia: e o que garante que o atalho antigo
//! e o daemon novo nunca pollem juntos, mesmo que alguem esqueca de
//! remover o atalho.
//!
//! ## Por que aqui e mais simples que no Linux
//!
//! O GNOME nao deixa ler a posicao do cursor: e preciso pedir uma
//! barreira ao compositor via `org.gnome.Mutter.InputCapture`, receber
//! sinal, e desarmar a barreira na mao porque o mutter ignora o recuo do
//! cursor. No Windows `GetCursorPos` responde direto -- 30 ms de loop e o
//! cursor e lido como ele esta.
//!
//! ## A permanencia se expressa diferente nas duas pontas
//!
//! O mutter entrega eventos discretos, entao do lado Linux a permanencia
//! e **densidade**: `hold_hits` ativacoes dentro de `hold_window_ms`.
//! Aqui medimos contato **continuo**, que e o que o polling de fato ve.
//! A traducao e: `hold_hits <= 1` significa trocar no primeiro toque (o
//! que o usuario pediu), e acima disso exigimos ficar na borda por
//! `hold_window_ms`. Mesma intencao, sinais diferentes.

use crate::config::Config;
use crate::focus::{self, FocusRequest};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(30);
const COOLDOWN: Duration = Duration::from_secs(10);
/// A presenca e HID++: a 33 Hz seria martelar o radio sem motivo. A
/// janela do dead-man's switch e de dezenas de segundos, 1 Hz basta.
const PRESENCE_EVERY: Duration = Duration::from_secs(1);
const GUARD_DEFAULT: u64 = 25;

pub fn spawn(cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || run(cfg));
}

fn run(cfg: Arc<RwLock<Config>>) {
    let (edge, target, hold) = {
        let Ok(g) = cfg.read() else { return };
        let b = &g.input.border;
        if !b.enabled {
            return;
        }
        let Some(t) = b.target.clone() else {
            gripe!("border: habilitado sem `target`; nada a fazer");
            return;
        };
        let hold = if b.hold_hits <= 1 {
            Duration::ZERO
        } else {
            Duration::from_millis(if b.hold_window_ms == 0 { 300 } else { b.hold_window_ms })
        };
        (b.edge.clone(), t, hold)
    };
    if !matches!(edge.as_str(), "left" | "right" | "top" | "bottom") {
        gripe!("border: edge '{edge}' invalido (left | right | top | bottom)");
        return;
    }

    // `GetCursorPos` e `GetLastInputInfo` sao POR SESSAO. Na sessao 0 --
    // o caso de lancar o daemon por SSH -- esta thread nao veria input
    // nenhum, leria um desktop falso de 1024x768 e ainda competiria pelo
    // receptor com a instancia de verdade. O resto do daemon (API, HID++)
    // funciona la, so a borda nao: por isso so esta thread desiste.
    if !winput::in_console_session() {
        gripe!(
            "border: fora da sessao do console -- cursor e input nao sao \
             visiveis daqui. O daemon segue servindo a API; a borda nao."
        );
        return;
    }
    if !winput::claim_single_instance("logid-agent-watch-edge") {
        gripe!(
            "border: ja existe outro vigia (daemon ou o atalho antigo de \
             watch-edge). Dois donos do mesmo receptor e problema real -- \
             esta thread desiste."
        );
        return;
    }

    let (vx, vy, vw, vh) = winput::virtual_screen();
    say!(
        "border: vigiando borda '{edge}' -> {target} (area {vx},{vy} {vw}x{vh}, \
         permanencia {}ms)",
        hold.as_millis()
    );

    let mut touching: Option<Instant> = None;
    let mut cooldown: Option<Instant> = None;
    let mut guard = Guard::new(&cfg);
    let mut last_presence = Instant::now() - PRESENCE_EVERY;
    // Ja comeca encostado? Entao nao conta como chegada. Sem isto, um
    // cursor estacionado na borda dispara no primeiro tick depois do
    // boot.
    let mut was_at_edge = winput::cursor().map(|p| winput::at_edge(&edge, p)).unwrap_or(false);
    if was_at_edge {
        say!("border: cursor ja esta na borda '{edge}' -- aguardando ele sair e voltar");
    }

    loop {
        std::thread::sleep(TICK);

        if let Some(t) = cooldown {
            if Instant::now() < t {
                continue;
            }
            cooldown = None;
            say!("border: cooldown vencido, vigiando de novo");
        }

        if last_presence.elapsed() >= PRESENCE_EVERY {
            last_presence = Instant::now();
            guard.tick(&cfg);
        }

        let Some(p) = winput::cursor() else { continue };
        if !winput::at_edge(&edge, p) {
            touching = None;
            was_at_edge = false;
            continue;
        }

        // Exige CHEGADA, nao presenca.
        //
        // O mutter entrega travessia como evento, entao do lado Linux isso
        // vem de graca. Aqui o polling le posicao: um cursor parado na
        // borda pareceria "tocando" a cada 30 ms, para sempre. O custo
        // disso nao e log repetido -- e que, no instante em que os
        // dispositivos CHEGASSEM nesta maquina, a borda os mandaria de
        // volta sem ninguem ter encostado em nada.
        //
        // Depois de disparar, `was_at_edge` fica `true`: so volta a valer
        // quando o cursor sair da borda e voltar.
        if was_at_edge && touching.is_none() {
            continue;
        }
        if !was_at_edge {
            was_at_edge = true;
            touching = Some(Instant::now());
        }
        let since = touching.unwrap_or_else(Instant::now);
        if since.elapsed() < hold {
            continue;
        }
        touching = None;
        cooldown = Some(Instant::now() + COOLDOWN);
        say!("border: borda '{edge}' alcancada em ({},{}) -- trocando", p.x, p.y);
        switch_away(&cfg, &target);
    }
}

/// A troca leva segundos (pre-flight + verificacao por efeito). Rodar no
/// loop travaria a leitura do cursor; vai para uma thread.
fn switch_away(cfg: &Arc<RwLock<Config>>, target: &str) {
    let Ok(snap) = cfg.read().map(|g| g.clone()) else { return };
    let host = target.to_string();
    std::thread::spawn(move || {
        // O pre-flight completo roda aqui dentro: config ruim nao pode te
        // deixar sem teclado so porque o mouse encostou na borda.
        let req = FocusRequest { host, route: None, confirm: true, fast: true };
        let outcome = focus::execute(&snap, &req);
        if outcome.executed {
            for r in &outcome.results {
                say!("border: {r}");
            }
        } else {
            let bloqueios: Vec<&str> = outcome
                .plan
                .checks
                .iter()
                .filter(|c| c.blocking && !c.ok)
                .map(|c| c.name.as_str())
                .collect();
            say!(
                "border: NAO trocou -- {} (bloqueios: {})",
                outcome.error.unwrap_or_else(|| "plano nao pronto".into()),
                bloqueios.join(", ")
            );
        }
    });
}

/// Dead-man's switch: quem detem o dispositivo e o unico que pode devolver.
///
/// A spec 005 exige isto e por muito tempo nao existiu de fato: eu tentei
/// lancar o guard por SSH e `GetLastInputInfo` nao ve a sessao do usuario
/// de fora dela. Aqui dentro funciona -- esta thread ja e residente na
/// sessao e ja faz polling.
///
/// Logica: quando os dispositivos CHEGAM nesta maquina, abrimos uma
/// janela. Se nenhum input aparecer nela, devolvemos.
struct Guard {
    secs: u64,
    peer: Option<String>,
    present_before: bool,
    arrived: Option<Instant>,
}

impl Guard {
    fn new(cfg: &Arc<RwLock<Config>>) -> Self {
        let secs = std::env::var("LOGID_GUARD_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(GUARD_DEFAULT);
        let peer = cfg.read().ok().and_then(|g| g.peer_host().map(|h| h.name.clone()));
        if peer.is_none() {
            gripe!(
                "border: sem host remoto na config -- o dead-man's switch \
                 nao sabe para onde devolver"
            );
        }
        Guard { secs, peer, present_before: false, arrived: None }
    }

    fn tick(&mut self, cfg: &Arc<RwLock<Config>>) {
        let Some(peer) = self.peer.clone() else { return };
        let wanted: Vec<String> = match cfg.read() {
            Ok(g) => g.devices.iter().map(|d| d.name.clone()).collect(),
            Err(_) => return,
        };
        if wanted.is_empty() {
            return;
        }

        // Reusa a varredura do daemon em vez de reabrir o receptor: o
        // refresher a mantem fresca e o cache evita trafego HID++ duplo.
        let survey = crate::hidpp::survey_cached_or_fresh(Duration::from_millis(2500));
        let present = survey.receivers.iter().any(|r| {
            r.devices.iter().any(|d| d.online && wanted.iter().any(|w| w == &d.name))
        });

        if present && !self.present_before {
            self.arrived = Some(Instant::now());
            say!(
                "border: dispositivos CHEGARAM -- vigiando input por {}s",
                self.secs
            );
        } else if !present && self.present_before {
            self.arrived = None;
            say!("border: dispositivos SAIRAM -- vigilancia encerrada");
        }
        self.present_before = present;

        let Some(t0) = self.arrived else { return };
        let Ok(idle) = winput::idle_ms() else { return };
        // Input novo: o idle caiu abaixo do tempo desde a chegada, logo
        // houve atividade depois dela.
        if idle < t0.elapsed().as_millis() as u64 {
            say!("border: input detectado -- dead-man's switch desarmado");
            self.arrived = None;
            return;
        }
        if t0.elapsed().as_secs() >= self.secs {
            say!(
                "border: SEM INPUT em {}s (idle {idle}ms) -- devolvendo para {peer}",
                self.secs
            );
            self.arrived = None;
            switch_away(cfg, &peer);
        }
    }
}
