//! Bandeja nativa, por StatusNotifierItem.
//!
//! Spec: ../../specs/001-control-plane.md
//!
//! ## Por que sair do Electron
//!
//! A bandeja morava em `gui/main.js`. Funcionava, mas carregava duas
//! consequencias que incomodam no uso diario:
//!
//! - **Dependia do Electron estar de pe.** Fechar a janela mantinha o
//!   processo so para o icone existir; mata-lo tirava a bandeja junto,
//!   e com ela o unico caminho de volta do KVM pelo menu.
//! - **Falava com o `logid` por HTTP** para ler estado e mandar o foco.
//!   Aqui a bandeja E o `logid`: chama `kvm::mandar` direto, sem
//!   serializar nada.
//!
//! O StatusNotifierItem e D-Bus puro -- nao precisa de GTK nem de laco
//! de eventos grafico, o que permite viver dentro de um daemon que ja
//! serve HTTP. No GNOME quem o desenha e a extensao
//! `ubuntu-appindicators`, a mesma que o Electron usava por baixo.
//!
//! ## Clique abre o menu
//!
//! Era pedido explicito: trocar de maquina por clique na bandeja, nao
//! por borda. O `activate` (clique esquerdo) devolve o menu, entao o
//! gesto e um clique e uma escolha -- sem arrastar o ponteiro para
//! canto nenhum. A borda continua no codigo, desligada por config.

#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};

use crate::config::Config;

/// O que a bandeja precisa saber, ja resolvido.
///
/// Fotografado por um vigia em outra thread em vez de consultado no
/// desenho do menu: ler HID++ custa ~360 ms, e um menu que trava um
/// terco de segundo ao abrir parece quebrado.
#[derive(Default, Clone)]
pub struct Foto {
    /// Linhas de "dispositivo: bateria", so para ler.
    pub dispositivos: Vec<String>,
    /// Hosts remotos (nome) para onde da para mandar o input.
    pub remotos: Vec<String>,
    /// Nome da maquina local, para o item de trazer de volta.
    pub aqui: Option<String>,
    /// Para onde o input esta indo agora, se nao e aqui.
    pub alvo: Option<String>,
}

static FOTO: Mutex<Option<Foto>> = Mutex::new(None);

pub fn atualizar(f: Foto) {
    if let Ok(mut g) = FOTO.lock() {
        *g = Some(f);
    }
}

fn foto() -> Foto {
    FOTO.lock().ok().and_then(|g| g.clone()).unwrap_or_default()
}

pub struct Bandeja {
    cfg: Arc<std::sync::RwLock<Config>>,
}

impl ksni::Tray for Bandeja {
    fn id(&self) -> String {
        "logid".into()
    }

    fn title(&self) -> String {
        "logid".into()
    }

    fn icon_name(&self) -> String {
        // Icone do tema, nao arquivo nosso: a bandeja tem de seguir o
        // tema claro/escuro do sistema, e um PNG fixo fica errado na
        // metade das vezes.
        "input-mouse".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let f = foto();
        let resumo = if f.dispositivos.is_empty() {
            "sem dispositivos".to_string()
        } else {
            f.dispositivos.join(" | ")
        };
        let onde = match &f.alvo {
            Some(a) => format!("input em {a}"),
            None => "input aqui".into(),
        };
        ksni::ToolTip {
            title: "logid".into(),
            description: format!("{onde}\n{resumo}"),
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    /// Clique esquerdo abre o menu, que e o gesto pedido.
    fn activate(&mut self, _x: i32, _y: i32) {}

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let f = foto();
        let mut itens: Vec<MenuItem<Self>> = Vec::new();

        if f.dispositivos.is_empty() {
            itens.push(
                StandardItem {
                    label: "nenhum dispositivo conhecido".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else {
            for d in &f.dispositivos {
                itens.push(
                    StandardItem { label: d.clone(), enabled: false, ..Default::default() }.into(),
                );
            }
        }

        if !f.remotos.is_empty() {
            itens.push(MenuItem::Separator);
            for nome in &f.remotos {
                let ativo = f.alvo.as_deref() == Some(nome.as_str());
                let alvo = nome.clone();
                itens.push(
                    StandardItem {
                        // O ponto marca para onde o input esta indo. Sem
                        // isso o menu nao distingue "posso mandar" de
                        // "ja esta la".
                        label: format!("{}Mandar para {nome}", if ativo { "● " } else { "" }),
                        activate: Box::new(move |b: &mut Bandeja| b.mandar(&alvo)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
            // "Trazer de volta" aparece quando o input esta fora, e
            // ponto. O menu do Electron condicionava a "algum
            // dispositivo offline", sinal que so vale no HID++ -- no
            // modo LAN o dispositivo nunca sai daqui, e o item sumia
            // justamente no modo em que ele e o unico caminho de volta.
            if let (Some(aqui), Some(_)) = (&f.aqui, &f.alvo) {
                let nome = aqui.clone();
                let rotulo = format!("Trazer para {aqui}");
                itens.push(
                    StandardItem {
                        label: rotulo,
                        activate: Box::new(move |b: &mut Bandeja| b.mandar(&nome)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        // Seleção grande de arquivos esperando destino.
        //
        // Acima do teto não mandamos para todo mundo sozinhos: copiar
        // 1,4 GB para duas máquinas que ninguém está olhando gasta
        // banda e disco alheio. Então o menu pergunta para onde, um
        // destino por vez. Ver [`crate::clip_arq::ao_copiar`].
        if let Some((caminhos, bytes)) = crate::clip_arq::oferta() {
            itens.push(MenuItem::Separator);
            itens.push(
                StandardItem {
                    label: format!(
                        "{} arquivo(s) copiado(s), {}",
                        caminhos.len(),
                        crate::clip_arq::humano(bytes)
                    ),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
            for nome in &f.remotos {
                let alvo = nome.clone();
                itens.push(
                    StandardItem {
                        label: format!("Mandar arquivos para {nome}"),
                        activate: Box::new(move |b: &mut Bandeja| b.mandar_arquivos(&alvo)),
                        ..Default::default()
                    }
                    .into(),
                );
            }
        }

        itens.push(MenuItem::Separator);
        itens.push(
            StandardItem {
                label: "Abrir painel".into(),
                activate: Box::new(|b: &mut Bandeja| b.abrir_painel()),
                ..Default::default()
            }
            .into(),
        );
        itens
    }
}

impl Bandeja {
    /// Manda a seleção grande de arquivos para um destino escolhido.
    ///
    /// Numa thread porque pode ser 1,4 GB: bloquear aqui travaria o
    /// menu inteiro -- e a bandeja roda na mesma thread que responde o
    /// DBusMenu, então o menu pararia de abrir.
    fn mandar_arquivos(&self, host: &str) {
        let Ok(g) = self.cfg.read() else { return };
        let snap = g.clone();
        drop(g);
        let alvo = host.to_string();
        std::thread::spawn(move || match crate::clip_arq::mandar_oferta(&snap, &alvo) {
            Ok(m) => crate::say!("bandeja: {m}"),
            Err(e) => crate::say!("bandeja: {e}"),
        });
    }

    fn mandar(&self, host: &str) {
        let snap = match self.cfg.read() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        // Direto, sem passar por HTTP: a bandeja E o logid.
        let r = if snap.input.mode == "lan" {
            crate::kvm::mandar(&snap, host).map(|d| format!("input para {host} ({d})"))
        } else {
            let req = crate::focus::FocusRequest {
                host: host.to_string(),
                route: None,
                confirm: true,
                fast: true,
            };
            let o = crate::focus::execute(&snap, &req);
            match o.error {
                Some(e) => Err(e),
                None => Ok(format!("foco para {host}")),
            }
        };
        match r {
            Ok(m) => crate::say!("bandeja: {m}"),
            Err(e) => crate::say!("bandeja: falhou mandar para {host}: {e}"),
        }
    }

    fn abrir_painel(&self) {
        let porta = self.cfg.read().map(|g| g.port).unwrap_or(8787);
        let _ = crate::sys::child("xdg-open")
            .arg(format!("http://127.0.0.1:{porta}/"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Sobe a bandeja e devolve o handle para atualizar o menu.
///
/// Falhar aqui **nao** e fatal: sem a extensao de AppIndicator (ou num
/// ambiente sem bandeja nenhuma) o item simplesmente nao aparece, e o
/// daemon continua servindo o painel e a API. Derrubar o `logid`
/// porque nao ha onde desenhar um icone seria trocar uma inconveniencia
/// por uma queda.
pub fn iniciar(cfg: Arc<std::sync::RwLock<Config>>) -> Option<ksni::blocking::Handle<Bandeja>> {
    use ksni::blocking::TrayMethods;
    let b = Bandeja { cfg };
    match b.spawn() {
        Ok(h) => {
            crate::say!("bandeja nativa no ar");
            Some(h)
        }
        Err(e) => {
            crate::say!("bandeja nativa nao subiu ({e}); seguindo sem ela");
            None
        }
    }
}

/// Mantem o menu fresco numa thread propria.
///
/// A foto e refeita a cada 5 s e so empurrada para a bandeja quando
/// MUDA. Chamar `update` a cada volta faria o shell redesenhar o menu
/// sem motivo -- e, num menu aberto, isso pisca na cara de quem esta
/// escolhendo.
pub fn vigiar(handle: ksni::blocking::Handle<Bandeja>, cfg: Arc<std::sync::RwLock<Config>>) {
    std::thread::spawn(move || {
        let mut anterior = String::new();
        loop {
            let nova = montar(&cfg);
            // Assinatura simples: o que o menu mostra, concatenado.
            // A oferta de arquivos entra na assinatura, e nao so na
            // montagem do menu: o host da bandeja CACHEIA o menu que a
            // gente empurra. Sem isso o item "Mandar arquivos" so
            // apareceria na proxima vez que outra coisa mudasse -- ou
            // nunca, numa maquina parada.
            let chave = format!(
                "{}|{}|{:?}|{:?}|{:?}",
                nova.dispositivos.join(","),
                nova.remotos.join(","),
                nova.aqui,
                nova.alvo,
                crate::clip_arq::oferta().map(|(c, b)| (c.len(), b))
            );
            if chave != anterior {
                anterior = chave;
                atualizar(nova);
                handle.update(|_| {});
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

fn montar(cfg: &Arc<std::sync::RwLock<Config>>) -> Foto {
    // Survey do cache: ler HID++ fresco custa ~360 ms, e a bandeja e
    // informativa, nao autoridade. Quem precisa de frescor e o disparo.
    let s = crate::hidpp::survey_cached_or_fresh(std::time::Duration::from_secs(20));
    let dispositivos = s
        .receivers
        .iter()
        .flat_map(|r| r.devices.iter())
        .map(|d| match d.battery_percent {
            Some(p) => format!("{}: {p}%", d.name),
            None => format!("{}: ?", d.name),
        })
        .collect();

    let (remotos, aqui) = match cfg.read() {
        Ok(g) => {
            let local = |h: &crate::config::HostCfg| {
                h.local.unwrap_or(false)
                    || matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1")
            };
            (
                g.hosts.iter().filter(|h| !local(h)).map(|h| h.name.clone()).collect(),
                g.hosts.iter().find(|h| local(h)).map(|h| h.name.clone()),
            )
        }
        Err(_) => (Vec::new(), None),
    };

    Foto { dispositivos, remotos, aqui, alvo: crate::kvm::alvo() }
}
