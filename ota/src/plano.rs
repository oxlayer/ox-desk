//! O que cada sistema exige para receber um build novo.
//!
//! Spec: ../../specs/015-ota.md
//!
//! Funcoes puras que devolvem **dados**: caminhos, nomes, o argv do
//! reinicio, e o motivo de recusa quando ha um. Quem executa e
//! `aplicar.rs`. A razao de separar e que as tres maquinas divergem em
//! tudo -- onde o binario vive, quem o reinicia, se precisa de
//! assinatura -- e essa divergencia precisa ser lida de uma vez, nao
//! garimpada em `cfg` espalhado.

use std::path::{Path, PathBuf};

/// Os tres sempre existem, mesmo em build de um deles so: a decisao de
/// cada sistema e lida de uma vez e **testada nas tres** desta maquina.
/// Um `cfg` por variante tornaria o plano do Mac indecifravel daqui.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sistema {
    Linux,
    Mac,
    Windows,
}

/// O sistema de quem esta rodando.
pub fn daqui() -> Sistema {
    #[cfg(target_os = "linux")]
    return Sistema::Linux;
    #[cfg(target_os = "macos")]
    return Sistema::Mac;
    #[cfg(windows)]
    return Sistema::Windows;
}

/// O comeco do nome do pacote de cada sistema.
///
/// Espelha o `PACOTE` do
/// [`construir-binarios.sh`](../../scripts/construir-binarios.sh). Se
/// alguem renomear la, aqui nao acha pacote e o estado diz exatamente
/// isso -- em vez de instalar o pacote de outra arquitetura.
pub fn prefixo(s: Sistema) -> &'static str {
    match s {
        Sistema::Linux => "logi-linux-x86_64-",
        Sistema::Mac => "logi-mac-arm64-unsigned-",
        Sistema::Windows => "logi-windows-x86_64-unsigned-",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plano {
    pub sistema: Sistema,
    /// O nome do daemon dentro do pacote. E ele que carrega o carimbo do
    /// commit e e ele que responde `/healthz` depois da troca.
    pub daemon: &'static str,
    /// Assinar o que foi extraido antes de instalar.
    ///
    /// So o Mac. E o passo que faz a concessao de Gravacao de Tela
    /// sobreviver: o TCC amarra a permissao ao *designated requirement*
    /// do binario, que a identidade estavel mantem igual. Binario
    /// ad-hoc da CI entraria com cdhash novo e perderia a permissao --
    /// e a maquina voltaria com tela preta.
    pub assina: bool,
    /// Por que este sistema **nao** troca binario sozinho, se for o caso.
    pub recusa: Option<&'static str>,
    /// O argv que reinicia o daemon.
    pub reiniciar: Vec<String>,
    /// O argv que tira da frente qualquer daemon que o supervisor **nao**
    /// conhece, antes de reiniciar.
    ///
    /// Existe porque o `kickstart -k` (e o `systemctl restart`) so matam
    /// a copia que o supervisor criou. Um `logid` orfao -- tipicamente
    /// subido pela GUI numa corrida de boot -- continua com a porta, o
    /// daemon novo morre com `Address already in use`, o `/healthz`
    /// segue respondendo o build VELHO, e este programa conclui que a
    /// atualizacao falhou e **reverte uma atualizacao correta**.
    ///
    /// Matar por nome exato nao e brutalidade: o `-k` do passo seguinte
    /// ja mata a copia supervisionada de qualquer forma, entao o efeito
    /// liquido desta linha e exatamente "tambem as que o supervisor nao
    /// conhece".
    pub limpar: Vec<String>,
}

pub fn plano(s: Sistema, uid: u32) -> Plano {
    match s {
        Sistema::Linux => Plano {
            sistema: s,
            daemon: "logid",
            assina: false,
            recusa: None,
            reiniciar: vec![
                "systemctl".into(),
                "--user".into(),
                "restart".into(),
                "logid.service".into(),
            ],
            limpar: vec!["pkill".into(), "-x".into(), "logid".into()],
        },
        Sistema::Mac => Plano {
            sistema: s,
            daemon: "logid",
            assina: true,
            recusa: None,
            reiniciar: vec![
                "launchctl".into(),
                "kickstart".into(),
                "-k".into(),
                format!("gui/{uid}/ai.logi.logid"),
            ],
            limpar: vec!["pkill".into(), "-x".into(), "logid".into()],
        },
        // A chave do Authenticode nao sai do Ubuntu (specs/008), e `.exe`
        // sem assinatura valida e o que o Defender apagou no instante da
        // escrita, duas vezes, levando a tarefa agendada junto. Entao
        // aqui o OTA **confere e relata**, e quem troca e o `deploy.sh`.
        //
        // Nao e perda grande: esta maquina nao viaja. O OTA existe para o
        // Mac, que viaja.
        Sistema::Windows => Plano {
            sistema: s,
            daemon: "logid.exe",
            assina: false,
            recusa: Some("nao assino aqui: a chave do Authenticode vive no Ubuntu (specs/008); rode o deploy"),
            reiniciar: vec![],
            limpar: vec![],
        },
    }
}

/// Onde os binarios instalados vivem.
///
/// `base` e `$HOME` no Unix e `%LOCALAPPDATA%` no Windows. Entra como
/// parametro para que isto seja testavel sem mexer no ambiente.
pub fn destino(s: Sistema, base: &Path) -> PathBuf {
    match s {
        Sistema::Linux | Sistema::Mac => base.join(".local").join("bin"),
        Sistema::Windows => base.join("logid"),
    }
}

/// Onde ficam o estado, o log e a copia do build anterior.
pub fn estado_dir(s: Sistema, base: &Path) -> PathBuf {
    match s {
        Sistema::Linux | Sistema::Mac => base.join(".local").join("state").join("logi-ota"),
        Sistema::Windows => base.join("logid").join("ota"),
    }
}

/// O script que assina, quando o sistema assina.
pub fn assinador(s: Sistema, base: &Path) -> Option<PathBuf> {
    match s {
        Sistema::Mac => Some(base.join(".local").join("bin").join("assinar.sh")),
        _ => None,
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn casa() -> PathBuf {
        PathBuf::from("/Users/x")
    }

    /// O Windows recusa, e **com motivo escrito**. Recusa sem motivo
    /// viraria "o OTA nao funciona no Windows" sem ninguem saber porque.
    #[test]
    fn windows_recusa_e_diz_por_que() {
        let p = plano(Sistema::Windows, 501);
        let r = p.recusa.expect("o Windows tem de recusar");
        assert!(r.contains("Authenticode"), "{r}");
        assert!(p.reiniciar.is_empty(), "quem recusa nao reinicia nada");
    }

    #[test]
    fn so_o_mac_assina_e_tem_assinador() {
        assert!(plano(Sistema::Mac, 501).assina);
        assert!(!plano(Sistema::Linux, 1000).assina);
        assert!(!plano(Sistema::Windows, 0).assina);
        assert!(assinador(Sistema::Mac, &casa()).is_some());
        assert!(assinador(Sistema::Linux, &casa()).is_none());
    }

    /// Quem aplica tem de ter como reiniciar. Um plano que troca o
    /// binario e nao reinicia deixa a maquina rodando o build velho com
    /// o arquivo novo no disco -- a divergencia mais dificil de ver.
    #[test]
    fn quem_aplica_sabe_reiniciar() {
        for s in [Sistema::Linux, Sistema::Mac, Sistema::Windows] {
            let p = plano(s, 501);
            if p.recusa.is_none() {
                assert!(!p.reiniciar.is_empty(), "{s:?} aplica e nao reinicia");
            }
        }
    }

    /// Quem aplica tem de saber tirar da frente o dono antigo da porta.
    /// Reiniciar sem isso foi o que deixou o Mac servindo build velho
    /// enquanto todo deploy dizia `ok`.
    #[test]
    fn quem_aplica_sabe_limpar_a_porta() {
        for s in [Sistema::Linux, Sistema::Mac] {
            let p = plano(s, 501);
            assert!(!p.limpar.is_empty(), "{s:?} aplica e nao limpa a porta");
            assert_eq!(p.limpar[0], "pkill", "{s:?}");
            // `-x`: nome EXATO. Sem ele o `pkill` casaria o proprio
            // `logi-ota` e qualquer linha de comando que mencione logid.
            assert!(p.limpar.contains(&"-x".to_string()), "{s:?}: {:?}", p.limpar);
        }
        assert!(plano(Sistema::Windows, 0).limpar.is_empty(), "quem recusa nao mata nada");
    }

    /// O `kickstart` do launchd precisa do uid no caminho do servico:
    /// `gui/501/...`. Sem ele o comando falha com "Could not find
    /// service", que parece servico ausente.
    #[test]
    fn o_reinicio_do_mac_leva_o_uid() {
        let p = plano(Sistema::Mac, 501);
        assert!(p.reiniciar.iter().any(|a| a == "gui/501/ai.logi.logid"), "{:?}", p.reiniciar);
    }

    #[test]
    fn caminhos_sao_absolutos_e_diferentes_por_sistema() {
        let d: Vec<PathBuf> = [Sistema::Linux, Sistema::Mac, Sistema::Windows]
            .iter()
            .map(|s| destino(*s, &casa()))
            .collect();
        for p in &d {
            assert!(p.is_absolute(), "{p:?}");
        }
        assert_eq!(d[0], d[1], "Linux e Mac usam o mesmo ~/.local/bin");
        assert_ne!(d[1], d[2], "o Windows nao usa .local/bin");
        assert!(estado_dir(Sistema::Mac, &casa()).ends_with("logi-ota"));
    }

    /// Prefixos distintos, e nenhum prefixo de outro. `logi-mac-arm64-`
    /// sem o `unsigned-` casaria um pacote assinado futuro, e isso e
    /// desejado -- mas nenhum pode casar o pacote de outro sistema.
    #[test]
    fn prefixos_nao_se_confundem() {
        let ps = [
            prefixo(Sistema::Linux),
            prefixo(Sistema::Mac),
            prefixo(Sistema::Windows),
        ];
        for (i, a) in ps.iter().enumerate() {
            for (j, b) in ps.iter().enumerate() {
                if i != j {
                    assert!(!a.starts_with(b), "{a} comeca com {b}");
                }
            }
        }
    }

    /// O daemon do Windows e `.exe`; o dos outros nao. Errar isto faria
    /// o carimbo nunca ser achado no pacote certo.
    #[test]
    fn o_nome_do_daemon_segue_o_sistema() {
        assert_eq!(plano(Sistema::Windows, 0).daemon, "logid.exe");
        assert_eq!(plano(Sistema::Linux, 0).daemon, "logid");
        assert_eq!(plano(Sistema::Mac, 0).daemon, "logid");
    }
}

/// Onde mora o token de leitura das Releases.
///
/// O repo e privado: sem token a API responde 200 com
/// `{"message":"Not Found"}`. O arquivo e um PAT de granulacao fina com
/// `Contents: read` **neste repo e so neste** -- ver ../../specs/015-ota.md
/// para o que isso muda no modelo de confianca.
pub fn token_path(s: Sistema, base: &Path) -> PathBuf {
    match s {
        Sistema::Linux | Sistema::Mac => base.join(".config").join("logi").join("ota-token"),
        Sistema::Windows => base.join("logid").join("ota-token"),
    }
}
