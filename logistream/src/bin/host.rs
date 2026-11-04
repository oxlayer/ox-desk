//! Host de tela: despacha para a implementacao desta plataforma.
//!
//! Spec: ../../specs/010-remote-desktop.md
//!
//! Um binario com o MESMO nome nas duas maquinas de proposito: quem
//! acorda o host (o painel, por SSH) nao precisa saber se do outro lado
//! ha PipeWire ou DXGI. O que muda e o segundo argumento -- conector de
//! video no Linux, indice de monitor no Windows -- porque a unidade de
//! "uma tela" e diferente nos dois, e inventar um nome comum seria
//! esconder isso.
//!
//!   logistream-host [porta] [conector|monitor]

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let porta: u16 = a.first().and_then(|v| v.parse().ok()).unwrap_or(9777);

    #[cfg(target_os = "linux")]
    {
        let conector = a.get(1).cloned().unwrap_or_else(|| "DP-1".into());
        logistream::host_linux::executar(porta, &conector)
    }

    #[cfg(windows)]
    {
        let monitor: u32 = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        logistream::host_win::executar(porta, monitor)
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (porta, a);
        anyhow::bail!(
            "este sistema ainda nao sabe servir a propria tela; aqui ele e o \
             cliente -- use `logistream-view`"
        )
    }
}
