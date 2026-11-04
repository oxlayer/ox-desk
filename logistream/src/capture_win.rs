//! Captura no Windows: DXGI Desktop Duplication.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Por que DXGI e nao GDI
//!
//! `BitBlt` do GDI copia a tela pela CPU, nao ve janelas aceleradas e
//! custa caro em 1440p. O Desktop Duplication entrega a textura que o
//! compositor **ja montou**, na GPU, e avisa quando nada mudou -- o que
//! significa zero trabalho com a tela parada. E o mesmo caminho que o
//! Parsec e o OBS usam.
//!
//! ## O cursor nao vem na imagem
//!
//! Esta e a diferenca visivel em relacao ao host do Linux, onde pedimos
//! ao Mutter `cursor-mode: 1` e ele embute o cursor no quadro. O DXGI
//! entrega a tela **sem** cursor e o ponteiro em separado
//! (`GetFramePointerShape`), porque e assim que o hardware compoe.
//!
//! Nao desenhamos o cursor por enquanto, e isso e menos grave do que
//! parece: o cliente manda posicao ABSOLUTA, entao o cursor da sua
//! propria maquina fica exatamente sobre o ponto remoto -- com zero
//! latencia, inclusive. O que falta e a FORMA (a barra de texto, as
//! setas de redimensionar). O Parsec faz igual por este motivo.
//!
//! ## A copia para a CPU e consciente
//!
//! O certo seria registrar a textura D3D11 direto no NVENC
//! (`NV_ENC_DEVICE_TYPE_DIRECTX`) e nunca sair da GPU. Fazemos a volta
//! pela CPU -- textura de staging, `Map`, upload para CUDA -- por um
//! motivo: e EXATAMENTE o caminho que o host do Linux ja faz (o PipeWire
//! entrega em memoria de CPU), entao o codificador e um so. Trocar isto
//! e otimizacao localizada aqui, nao mudanca de arquitetura.

#![cfg(windows)]

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

/// O que saiu de uma tentativa de pegar quadro.
#[derive(Debug, PartialEq, Eq)]
pub enum Estado {
    /// Imagem nova na textura de staging; da para mapear e codificar.
    Novo,
    /// Nada mudou dentro do prazo. Tela parada e o caso comum, nao erro.
    Parado,
    /// A duplicacao morreu -- mudanca de resolucao, tela de UAC, jogo em
    /// tela cheia assumindo o display. Tem de recriar.
    Perdido,
}

/// Pixels mapeados da textura de staging. Desmapeia ao sair de escopo.
///
/// Guard e nao `(ponteiro, tamanho)` porque esquecer o `Unmap` trava a
/// textura para sempre e o proximo `CopyResource` falha calado.
pub struct Mapeado<'a> {
    contexto: &'a ID3D11DeviceContext,
    recurso: ID3D11Resource,
    pub px: &'a [u8],
    pub stride: usize,
}

impl Drop for Mapeado<'_> {
    fn drop(&mut self) {
        unsafe { self.contexto.Unmap(&self.recurso, 0) };
    }
}

/// O monitor, antes de a duplicacao comecar.
///
/// Separado de `Tela` por uma razao medida: segurar a duplicacao do DXGI
/// enquanto o CUDA inicializa **travava o processo**. O log parava em
/// "DXGI: monitor 0, 2560x1440" com 0,2 s de CPU e ficava ali -- a mesma
/// sonda de NVENC, sozinha, rodava a 111 fps na mesma sessao. Entao a
/// ordem passou a ser: abrir o monitor, ler o tamanho, montar o encoder,
/// e SO DEPOIS duplicar.
///
/// De quebra a ordem nova e melhor desenho: se o NVENC nao estiver
/// disponivel, falhamos antes de tomar a tela de alguem.
pub struct Monitor {
    dispositivo: ID3D11Device,
    contexto: ID3D11DeviceContext,
    saida: IDXGIOutput1,
    w: u32,
    h: u32,
    /// Canto do monitor no desktop virtual. O input precisa dele para
    /// converter pixel do stream em posicao absoluta da maquina.
    x: i32,
    y: i32,
}

impl Monitor {
    /// Abre o monitor `indice` (0 = primario) sem comecar a capturar.
    pub fn abrir(indice: u32) -> anyhow::Result<Self> {
        unsafe {
            let mut dispositivo: Option<ID3D11Device> = None;
            let mut contexto: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                // BGRA_SUPPORT: o desktop e BGRA, e sem a flag a criacao
                // da textura de staging pode ser recusada no formato.
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut dispositivo),
                None,
                Some(&mut contexto),
            )?;
            let dispositivo = dispositivo
                .ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice nao devolveu dispositivo"))?;
            let contexto = contexto
                .ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice nao devolveu contexto"))?;

            // Do dispositivo para o adaptador, do adaptador para a saida:
            // e preciso duplicar a saida do MESMO adaptador que criou o
            // dispositivo, senao a textura atravessaria placas.
            let dxgi: IDXGIDevice = dispositivo.cast()?;
            let adaptador = dxgi.GetAdapter()?;
            let saida = adaptador
                .EnumOutputs(indice)
                .map_err(|e| anyhow::anyhow!("monitor {indice}: {e}"))?;
            let saida: IDXGIOutput1 = saida.cast()?;
            let desc = saida.GetDesc()?;
            let r = desc.DesktopCoordinates;
            let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
            println!("  DXGI: monitor {indice}, {w}x{h}");
            Ok(Monitor { dispositivo, contexto, saida, w, h, x: r.left, y: r.top })
        }
    }

    pub fn tamanho(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Canto superior esquerdo do monitor dentro do desktop virtual.
    pub fn origem(&self) -> (i32, i32) {
        (self.x, self.y)
    }

    /// Comeca a capturar. A partir daqui o compositor esta nos esperando
    /// a cada quadro, entao tudo que e lento tem de estar pronto.
    pub fn duplicar(self) -> anyhow::Result<Tela> {
        unsafe {
            let dup = self
                .saida
                .DuplicateOutput(&self.dispositivo)
                .map_err(|e| anyhow::anyhow!("DuplicateOutput: {e}"))?;
            let staging = cria_staging(&self.dispositivo, self.w, self.h)?;
            Ok(Tela { m: self, dup, staging, tem_imagem: false })
        }
    }
}

pub struct Tela {
    m: Monitor,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    /// Houve ao menos um quadro: sem isto, mapear leria lixo.
    tem_imagem: bool,
}

impl Tela {
    pub fn tamanho(&self) -> (u32, u32) {
        self.m.tamanho()
    }

    pub fn origem(&self) -> (i32, i32) {
        self.m.origem()
    }

    pub fn tem_imagem(&self) -> bool {
        self.tem_imagem
    }

    /// Tenta pegar um quadro novo, esperando no maximo `espera_ms`.
    ///
    /// Em caso de `Novo`, a imagem fica na textura de staging e sai por
    /// `mapear`. A separacao existe para o `ReleaseFrame` acontecer
    /// **antes** do `Map`: enquanto o quadro nao e devolvido, o
    /// compositor espera por nos.
    pub fn proximo(&mut self, espera_ms: u32) -> anyhow::Result<Estado> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut recurso: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(espera_ms, &mut info, &mut recurso) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Estado::Parado),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => return Ok(Estado::Perdido),
                Err(e) => return Err(anyhow::anyhow!("AcquireNextFrame: {e}")),
            }

            // `LastPresentTime == 0` quer dizer que so o ponteiro se
            // moveu -- a imagem do desktop e a mesma. Codificar de novo
            // gastaria GPU para transmitir o quadro que o cliente ja tem.
            let so_ponteiro = info.LastPresentTime == 0;
            let estado = if so_ponteiro { Estado::Parado } else { Estado::Novo };

            if !so_ponteiro {
                if let Some(r) = &recurso {
                    let tex: ID3D11Texture2D = r.cast()?;
                    self.m.contexto.CopyResource(&self.staging, &tex);
                    self.tem_imagem = true;
                }
            }
            // Sempre: devolver o quadro e obrigacao, mesmo quando so o
            // ponteiro mudou. Nao devolver trava a captura no proximo
            // Acquire com "ja existe quadro em aberto".
            let _ = self.dup.ReleaseFrame();
            Ok(estado)
        }
    }

    /// Mapeia a ultima imagem copiada para leitura pela CPU.
    ///
    /// Serve tanto para o quadro que acabou de chegar quanto para
    /// reenviar o ultimo com a tela parada -- o staging guarda a imagem,
    /// entao reenviar nao custa copia nova.
    pub fn mapear(&self) -> anyhow::Result<Mapeado<'_>> {
        unsafe {
            let recurso: ID3D11Resource = self.staging.cast()?;
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.m.contexto.Map(&recurso, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let stride = m.RowPitch as usize;
            let px = std::slice::from_raw_parts(m.pData as *const u8, stride * self.m.h as usize);
            Ok(Mapeado { contexto: &self.m.contexto, recurso, px, stride })
        }
    }

    /// Recria a duplicacao depois de `Perdido`.
    ///
    /// A resolucao pode ter mudado junto (foi o que derrubou a anterior),
    /// entao a textura de staging e o tamanho sao refeitos tambem --
    /// devolve `true` quando quem chama precisa refazer o codificador.
    pub fn recriar(&mut self) -> anyhow::Result<bool> {
        unsafe {
            let desc = self.m.saida.GetDesc()?;
            let r = desc.DesktopCoordinates;
            let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
            self.dup = self
                .m
                .saida
                .DuplicateOutput(&self.m.dispositivo)
                .map_err(|e| anyhow::anyhow!("DuplicateOutput: {e}"))?;
            // A origem pode mudar sem o tamanho mudar (monitor arrastado
            // para o outro lado no painel de controle), entao ela e
            // atualizada fora do `if`.
            self.m.x = r.left;
            self.m.y = r.top;
            let mudou = (w, h) != (self.m.w, self.m.h);
            if mudou {
                self.staging = cria_staging(&self.m.dispositivo, w, h)?;
                self.m.w = w;
                self.m.h = h;
                self.tem_imagem = false;
                println!("  DXGI: resolucao mudou para {w}x{h}");
            }
            Ok(mudou)
        }
    }
}

/// Textura que a CPU consegue ler. `STAGING` + `CPU_ACCESS_READ` e a
/// unica combinacao que o D3D11 deixa mapear para leitura.
fn cria_staging(d: &ID3D11Device, w: u32, h: u32) -> anyhow::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut t: Option<ID3D11Texture2D> = None;
    unsafe { d.CreateTexture2D(&desc, None, Some(&mut t))? };
    t.ok_or_else(|| anyhow::anyhow!("CreateTexture2D nao devolveu textura"))
}
