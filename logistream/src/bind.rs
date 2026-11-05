//! Em qual endereço o host escuta.
//!
//! Spec: ../../specs/012-fora-da-lan.md
//!
//! ## Por que isto existe
//!
//! O host sempre escutou em `0.0.0.0`, e dentro de casa isso estava
//! dentro do trust model da spec 008: "tudo na LAN". Só que o canal de
//! tela **cifra e não autentica** -- o cliente aceita qualquer
//! certificado (`AceitaQualquer` em `net.rs`, marcado como temporário
//! desde que foi escrito). Quem alcança a 9777 assiste à tela, e quem
//! alcança a 9778 digita nela.
//!
//! Enquanto as três máquinas ficavam em casa, o alcance era a LAN de
//! casa. Levar o MacBook para fora muda isso: num Wi-Fi de hotel,
//! `0.0.0.0` serve a tela e o teclado para a rede inteira do hotel.
//!
//! ## A regra que decide o desenho
//!
//! `overlay` **falha** quando não acha o endereço do overlay. Não cai
//! para `0.0.0.0`.
//!
//! A queda silenciosa seria confortável e seria exatamente o bug: a
//! pessoa pediu "só pelo overlay", o Tailscale não subiu, e o host
//! abriria para o Wi-Fi do hotel sem ninguém saber. É a mesma família
//! do `backfill_gate` que não gateava e do `catch` que engolia o
//! `MODULE_NOT_FOUND`. Erro barulhento > proteção imaginária.
//!
//! ## Por que não basta olhar a faixa 100.64.0.0/10
//!
//! Essa faixa é a CGNAT (RFC 6598), e o Tailscale usa ela de propósito.
//! Mas ela é CGNAT de verdade também: um celular compartilhando a
//! internet entrega endereço dessa faixa para quem conecta. O MacBook
//! em hotspot -- que é justamente o caso de teste -- teria um `en0` em
//! 100.x e o host acharia que era o overlay, escutando na rede do
//! celular.
//!
//! Por isso o critério é **faixa + nome da interface**.

use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modo {
    /// `0.0.0.0` -- o comportamento de sempre. Dentro de casa, na LAN.
    Todas,
    /// Só o endereço do overlay. Para a máquina que sai de casa.
    Overlay,
    /// Um endereço escolhido à mão.
    Fixo(IpAddr),
}

impl Modo {
    pub fn parse(s: &str) -> Result<Modo, String> {
        match s {
            "todas" | "all" | "0.0.0.0" => Ok(Modo::Todas),
            "overlay" | "tailscale" => Ok(Modo::Overlay),
            outro => outro
                .parse::<IpAddr>()
                .map(Modo::Fixo)
                .map_err(|_| format!("--bind: '{outro}' não é 'todas', 'overlay' nem um endereço")),
        }
    }
}

/// Endereço do overlay, dentre as interfaces desta máquina.
///
/// `interfaces` é `(nome, endereço)`. Puro para ser testável: a lista
/// real vem de `locais()`, e os casos que importam -- hotspot com
/// CGNAT, Tailscale fora do ar, duas interfaces candidatas -- não
/// precisam de máquina nenhuma para serem exercitados.
pub fn achar_overlay(interfaces: &[(String, IpAddr)]) -> Result<IpAddr, String> {
    let candidatos: Vec<&(String, IpAddr)> =
        interfaces.iter().filter(|(nome, ip)| e_do_overlay(nome, ip)).collect();
    if candidatos.is_empty() {
        return Err(
            "não achei endereço de overlay nesta máquina. O Tailscale está conectado? \
             (`tailscale status`). NÃO vou escutar em 0.0.0.0 no lugar."
                .into(),
        );
    }

    // IPv4 primeiro, e isto NÃO é preferência estética.
    //
    // A primeira versão exigia um candidato só e errava em toda
    // máquina real: o Tailscale dá v4 **e** v6 na mesma interface, e
    // "achei 2 endereços de overlay" me chegou do Windows de verdade
    // (Tailscale=fd7a:115c:a1e0::ab12:cd34 e Tailscale=100.88.10.20).
    // Ou seja, `--bind overlay` nunca teria funcionado em lugar nenhum
    // -- e os testes não pegaram porque eu escrevi os dois casos com
    // duas INTERFACES, que é o caso raro, em vez de uma interface com
    // as duas famílias, que é o caso normal.
    //
    // v4 porque um socket UDP ligado a um endereço v6 não recebe
    // tráfego v4, e os pares vão falar v4 por padrão.
    let v4: Vec<&&(String, IpAddr)> = candidatos.iter().filter(|(_, ip)| ip.is_ipv4()).collect();
    match v4.len() {
        1 => return Ok(v4[0].1),
        // Dois v4 de overlay é estado que não devia existir, e escolher
        // em silêncio seria escolher errado metade das vezes.
        n if n > 1 => {
            return Err(format!(
                "achei {n} endereços IPv4 de overlay ({}); use --bind <endereço> para decidir",
                lista(&v4.iter().map(|x| **x).collect::<Vec<_>>())
            ))
        }
        _ => {}
    }

    // Só IPv6: tailnet sem v4 é possível. Um candidato serve; vários,
    // não dá para adivinhar.
    match candidatos.len() {
        1 => Ok(candidatos[0].1),
        n => Err(format!(
            "achei {n} endereços IPv6 de overlay ({}); use --bind <endereço> para decidir",
            lista(&candidatos)
        )),
    }
}

fn lista(v: &[&(String, IpAddr)]) -> String {
    v.iter().map(|(n, ip)| format!("{n}={ip}")).collect::<Vec<_>>().join(", ")
}

/// Esta interface é o overlay?
///
/// Faixa **e** nome. Os nomes, por sistema:
///
/// | SO | interface |
/// |---|---|
/// | Linux | `tailscale0` |
/// | macOS | `utun3`, `utun4`... (o índice varia a cada boot) |
/// | Windows | `Tailscale` (nome amigável do adaptador) |
///
/// No macOS o nome sozinho não serve: VPN corporativa, iCloud Private
/// Relay e o próprio sistema criam `utun`. É o par faixa+nome que
/// identifica.
fn e_do_overlay(nome: &str, ip: &IpAddr) -> bool {
    if !na_faixa_do_tailscale(ip) {
        return false;
    }
    let n = nome.to_ascii_lowercase();
    n.starts_with("tailscale") || n.starts_with("utun") || n.contains("tailscale")
}

/// 100.64.0.0/10 (IPv4) ou fd7a:115c:a1e0::/48 (IPv6).
fn na_faixa_do_tailscale(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..=127).contains(&o[1])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
    }
}

/// O endereço final, já resolvido.
pub fn resolver(modo: &Modo, interfaces: &[(String, IpAddr)]) -> Result<IpAddr, String> {
    match modo {
        Modo::Todas => Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        Modo::Fixo(ip) => Ok(*ip),
        Modo::Overlay => achar_overlay(interfaces),
    }
}

/// As interfaces desta máquina, de verdade.
pub fn locais() -> Vec<(String, IpAddr)> {
    match if_addrs::get_if_addrs() {
        Ok(v) => v.into_iter().map(|i| (i.name, i.addr.ip())).collect(),
        Err(e) => {
            eprintln!("  nao consegui listar interfaces: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn i(nome: &str, ip: &str) -> (String, IpAddr) {
        (nome.into(), ip.parse().unwrap())
    }

    #[test]
    fn acha_o_tailscale_no_linux() {
        let ifs = [i("lo", "127.0.0.1"), i("eno1", "192.168.1.50"), i("tailscale0", "100.101.102.103")];
        assert_eq!(achar_overlay(&ifs).unwrap(), "100.101.102.103".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn acha_o_tailscale_no_mac() {
        // O indice do utun muda a cada boot; casar "utun3" fixo seria
        // funcionar hoje e falhar amanha.
        let ifs = [i("en0", "192.168.1.60"), i("utun7", "100.72.1.9")];
        assert_eq!(achar_overlay(&ifs).unwrap(), "100.72.1.9".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn hotspot_com_cgnat_nao_e_overlay() {
        // ESTE e o caso que a faixa sozinha erraria, e e justamente o
        // cenario de teste de amanha: MacBook preso no celular. O `en0`
        // ganha endereco 100.x da operadora, e escutar nele seria servir
        // a tela para a rede do celular.
        let ifs = [i("en0", "100.90.1.5"), i("lo0", "127.0.0.1")];
        assert!(achar_overlay(&ifs).is_err());
    }

    #[test]
    fn sem_overlay_falha_e_nao_cai_para_todas() {
        // A regra inteira deste modulo em um teste: `resolver` com
        // Overlay e sem overlay tem de ERRAR. Se um dia alguem "melhorar"
        // isto com um fallback, o teste quebra -- e e para quebrar.
        let ifs = [i("en0", "192.168.1.60")];
        let e = resolver(&Modo::Overlay, &ifs).unwrap_err();
        assert!(e.contains("não achei"), "{e}");
        assert!(!e.contains("0.0.0.0") || e.contains("NÃO vou"), "{e}");
    }

    #[test]
    fn v4_e_v6_na_mesma_interface_e_o_caso_NORMAL() {
        // O caso que a primeira versao tratava como erro, e que chegou
        // do Windows de verdade: o Tailscale da as duas familias na
        // mesma interface. Exigir "um candidato so" fazia
        // `--bind overlay` falhar em TODA maquina.
        let ifs = [
            i("Tailscale", "fd7a:115c:a1e0::ab12:cd34"),
            i("Tailscale", "100.88.10.20"),
            i("Ethernet", "192.168.1.42"),
        ];
        assert_eq!(achar_overlay(&ifs).unwrap(), "100.88.10.20".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn so_ipv6_ainda_serve() {
        let ifs = [i("tailscale0", "fd7a:115c:a1e0::1234")];
        assert_eq!(achar_overlay(&ifs).unwrap(), "fd7a:115c:a1e0::1234".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn dois_v4_de_overlay_pedem_decisao() {
        let ifs = [i("tailscale0", "100.1.1.1"), i("utun5", "100.2.2.2")];
        assert!(achar_overlay(&ifs).is_err());
    }

    #[test]
    fn faixa_vizinha_nao_conta() {
        // 100.63 e 100.128 estao FORA do /10. Errar o calculo da
        // mascara aqui abriria a maquina numa faixa publica.
        assert!(!na_faixa_do_tailscale(&"100.63.0.1".parse().unwrap()));
        assert!(!na_faixa_do_tailscale(&"100.128.0.1".parse().unwrap()));
        assert!(na_faixa_do_tailscale(&"100.64.0.1".parse().unwrap()));
        assert!(na_faixa_do_tailscale(&"100.127.255.254".parse().unwrap()));
    }

    #[test]
    fn modo_parseia_as_tres_formas() {
        assert_eq!(Modo::parse("todas").unwrap(), Modo::Todas);
        assert_eq!(Modo::parse("overlay").unwrap(), Modo::Overlay);
        assert_eq!(Modo::parse("10.0.0.5").unwrap(), Modo::Fixo("10.0.0.5".parse().unwrap()));
        assert!(Modo::parse("sim").is_err());
    }

    #[test]
    fn todas_continua_sendo_o_padrao_de_hoje() {
        assert_eq!(resolver(&Modo::Todas, &[]).unwrap(), "0.0.0.0".parse::<IpAddr>().unwrap());
    }
}

// ------------------------------------------------- escolha do processo
//
// Resolvido UMA vez, na partida, e lido pelos três hosts. A alternativa
// -- carregar um `IpAddr` por três assinaturas de plataforma que hoje
// recebem `(porta, tela)` -- espalharia o parâmetro por código que não
// tem nada a ver com rede. Aqui é configuração do processo, decidida
// antes de qualquer socket existir.

use std::sync::OnceLock;

static ESCUTA: OnceLock<IpAddr> = OnceLock::new();

/// Fixa o endereço de escuta. Chamar antes de abrir qualquer socket.
pub fn definir(ip: IpAddr) {
    let _ = ESCUTA.set(ip);
}

/// O endereço de escuta. `0.0.0.0` quando ninguém escolheu -- que é o
/// comportamento que o host sempre teve.
pub fn escuta() -> IpAddr {
    *ESCUTA.get().unwrap_or(&IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}
