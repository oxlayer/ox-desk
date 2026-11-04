//! KVM por LAN: manda teclado e mouse para outra maquina pela rede.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! A alternativa ao HID++ 0x1814. Aquele manda o PROPRIO dispositivo
//! trocar de canal Easy-Switch, o que exige um receptor pareado na
//! maquina de destino. Com um dongle so -- que foi onde paramos depois
//! que dois dongles perto se provaram instaveis -- apertar "mandar para
//! windows-host" fazia o teclado procurar um receptor que nao existe la,
//! nao achar, e voltar. Era o comportamento correto do protocolo
//! resolvendo o problema errado.
//!
//! Aqui o dispositivo nao sai do lugar: capturamos localmente
//! (`EVIOCGRAB`) e injetamos na outra ponta pelo mesmo stream de input
//! que a tela remota ja usa.
//!
//! ## Um processo filho, nao uma thread
//!
//! O `logistream-kvm` roda como processo separado, e isso e desenho,
//! nao preguica: **matar o processo solta o grab**, porque o kernel
//! solta quando o fd fecha. Se o KVM fosse uma thread do `logid` e ela
//! travasse, o teclado ficaria agarrado e a unica saida seria matar o
//! agente inteiro -- que e a coisa que voce usa para consertar as
//! outras. Processo filho faz da desistencia uma operacao de uma linha.

use std::process::{Child, Stdio};
use std::sync::Mutex;

use crate::config::Config;

/// O filho vivo e para quem ele esta mandando.
static ATUAL: Mutex<Option<(String, Child)>> = Mutex::new(None);

/// Para quem o input esta indo agora, ou `None` se esta aqui.
pub fn alvo() -> Option<String> {
    let mut g = ATUAL.lock().ok()?;
    // Colher o filho antes de responder: se ele morreu (socorro pelos
    // dois Ctrl, alvo caiu, rede sumiu), o input ja voltou para esta
    // maquina e dizer o contrario deixaria a bandeja mentindo.
    if let Some((_, c)) = g.as_mut() {
        if matches!(c.try_wait(), Ok(Some(_))) {
            *g = None;
        }
    }
    g.as_ref().map(|(n, _)| n.clone())
}

/// Traz o input de volta para esta maquina. Idempotente.
pub fn soltar() -> Result<(), String> {
    let mut g = ATUAL.lock().map_err(|_| "estado do KVM travado")?;
    if let Some((nome, mut c)) = g.take() {
        let _ = c.kill();
        let _ = c.wait();
        say!("kvm: input de volta (estava em {nome})");
    }
    Ok(())
}

/// Manda o input para `host`. Solta o alvo anterior, se houver.
///
/// A ordem importa: soltamos o anterior ANTES de tentar o novo. Dois
/// `EVIOCGRAB` no mesmo dispositivo nao coexistem, e o segundo falharia
/// com "Device or resource busy" -- deixando o input preso no destino
/// velho e a mensagem de erro falando de outra coisa.
pub fn mandar(cfg: &Config, host: &str) -> Result<String, String> {
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' nao esta na config"))?;
    // Mandar para a maquina local = trazer de volta. Mesmo teste de
    // "e aqui?" que o focus.rs usa, e nao so a flag `local`: ela e
    // opcional na config e nem todo host a declara, entao confiar so
    // nela faria "Trazer para ubuntu-host" tentar abrir um KVM
    // apontando para nos mesmos.
    let aqui = h.local.unwrap_or(false)
        || matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1");
    if aqui {
        return soltar().map(|()| "input local".to_string());
    }
    let sc = h
        .screen
        .as_ref()
        .ok_or_else(|| format!("'{host}' nao tem `[hosts.screen]`: sem porta para mandar input"))?;

    // Recusar ANTES de agarrar o que nao sabe receber.
    //
    // O host do macOS captura tela mas nao injeta input -- ele registra
    // o evento e ignora. Mandar o KVM para la agarrava o teclado da
    // pessoa e nao fazia nada do outro lado: o pior modo de falha
    // possivel, porque parece que o programa travou a maquina sem
    // motivo. Foi o que o usuario viveu ("para macbook air nao
    // funcionou tambem").
    //
    // Enquanto nao houver injecao por CGEvent la, e melhor dizer nao.
    if h.os.as_deref() == Some("macos") {
        return Err(format!(
            "'{host}' roda macOS, e o host de la ainda nao injeta input -- \
             agarrar o teclado para mandar eventos que ninguem aplica \
             deixaria voce sem teclado e sem efeito. Falta a injecao por \
             CGEvent (precisa de permissao de Acessibilidade)."
        ));
    }

    soltar()?;

    // Levantar o host da outra ponta primeiro. E ele quem recebe o
    // stream de input; sem ele de pe o `logistream-kvm` recusa conectar
    // -- e recusar e o comportamento certo, porque agarrar o teclado
    // sem destino deixaria a pessoa sem teclado E sem para onde ir.
    crate::screen::acordar_host(h, sc.port)
        .map_err(|e| format!("nao consegui levantar o host de {host}: {e}"))?;

    // `porta + 1`: a porta de INPUT do host, nao a de video.
    //
    // O host serve um cliente de video por vez. Enquanto o KVM
    // conectava na porta de video, ele tomava a vaga do visualizador --
    // o input chegava e era aplicado, mas a tela que a pessoa estava
    // olhando congelava, e a conclusao obvia era "nao funcionou".
    // Portas separadas tambem evitam a maquina alvo codificar video
    // para um cliente que nunca desenha nada.
    let destino = format!("{}:{}", h.addr, sc.port + 1);

    // Saida do filho num arquivo, nao em /dev/null.
    //
    // A primeira versao jogava fora stdout e stderr, como o lancamento
    // do cliente de tela faz. Mas aquele abre uma JANELA: se ele morre,
    // a pessoa ve. Este nao abre nada -- o unico sinal de que falhou
    // seria o teclado continuar aqui, sem uma linha dizendo por que.
    let registro = crate::sys::state_dir().join("logistream-kvm.log");
    let saida = std::fs::File::create(&registro)
        .map_err(|e| format!("nao consegui abrir {}: {e}", registro.display()))?;
    let erro = saida
        .try_clone()
        .map_err(|e| format!("nao consegui duplicar o descritor: {e}"))?;

    // Alvo Linux aplica a propria repeticao de tecla (o Mutter repete
    // teclas injetadas), entao mandar a do kernel junto dobraria. No
    // Windows e o contrario: `SendInput` nao repete nada, e sem a do
    // kernel segurar backspace apaga um caractere so.
    let mut cmd = crate::sys::child("logistream-kvm");
    cmd.arg(&destino);
    if h.os.as_deref() == Some("linux") {
        cmd.arg("--sem-repeticao");
    }
    let mut filho = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(saida))
        .stderr(Stdio::from(erro))
        .spawn()
        .map_err(|e| format!("nao consegui executar logistream-kvm: {e}"))?;

    // Confirmar que ele SOBREVIVEU, nao so que nasceu.
    //
    // `spawn` devolve Ok assim que o processo existe -- antes de ele
    // conectar e antes de agarrar. Reportar sucesso ali faria a bandeja
    // dizer "mandado para windows-host" enquanto o filho ja morreu porque
    // o alvo recusou a conexao ou faltou ACL no /dev/input. O estado se
    // corrigiria sozinho no proximo `alvo()`, mas o usuario ja teria
    // lido a mentira e estaria esperando o teclado mudar de maquina.
    //
    // 500 ms cobrem conectar + agarrar numa LAN; o caminho feliz nao
    // sente, e o caminho triste ganha a mensagem de erro de verdade.
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Ok(Some(status)) = filho.try_wait() {
        let motivo = std::fs::read_to_string(&registro)
            .unwrap_or_default()
            .lines()
            .last()
            .unwrap_or("sem detalhe no log")
            .trim()
            .to_string();
        return Err(format!("o KVM saiu ({status}) antes de agarrar: {motivo}"));
    }

    match ATUAL.lock() {
        Ok(mut g) => *g = Some((host.to_string(), filho)),
        Err(_) => return Err("estado do KVM travado".into()),
    }
    say!("kvm: input indo para {host} ({destino})");
    Ok(destino)
}
