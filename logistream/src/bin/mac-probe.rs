//! O que o Mac oferece para virar HOST de tela.
//!
//! Spec: ../../specs/010-remote-desktop.md
//!
//! Primeiro passo do host do Mac, no mesmo espirito do que fiz para o
//! Windows: provar as pecas antes de montar o pipeline. A pergunta que
//! este probe responde e uma so, e e a que decide o desenho: **o
//! libavdevice linka e o encoder por hardware existe?**
//!
//! Se sim, o host do Mac sai sem FFI novo -- captura pelo dispositivo
//! `avfoundation` e codificacao pelo `h264_videotoolbox`, os dois no
//! ffmpeg que ja esta instalado. Se nao, o caminho e ScreenCaptureKit e
//! VideoToolbox crus, que sao centenas de linhas de ponte com
//! Objective-C para chegar no mesmo lugar.
//!
//!   logistream-probe-mac

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    ffmpeg_next::init()?;
    // Registra os dispositivos de entrada. Se isto linka, o `avdevice`
    // esta no binario -- que e metade da pergunta.
    ffmpeg_next::device::register_all();

    println!("  libavdevice: linkado e registrado");

    let mut achou = false;
    for nome in ["h264_videotoolbox", "libx264", "h264"] {
        match ffmpeg_next::encoder::find_by_name(nome) {
            Some(_) => {
                println!("  encoder {nome}: existe");
                achou = true;
            }
            None => println!("  encoder {nome}: ausente"),
        }
    }
    if !achou {
        anyhow::bail!("nenhum encoder H.264 neste ffmpeg");
    }

    // Entrada de captura: perguntar pelo NOME, nao enumerar.
    //
    // `format::list()` existia e foi removido no ffmpeg 5; a crate so o
    // expoe para versoes antigas. O caminho atual e o mesmo que o
    // proprio `ffmpeg -f avfoundation` usa: procurar o demuxer pelo
    // nome. Ponteiro nao nulo prova que o dispositivo esta compilado na
    // biblioteca E que o `avdevice` foi registrado -- sem ele o nome nao
    // resolve.
    let nome = std::ffi::CString::new("avfoundation")?;
    let encontrado = unsafe { ffmpeg_next::ffi::av_find_input_format(nome.as_ptr()) };
    if encontrado.is_null() {
        println!("  entrada avfoundation: AUSENTE");
        anyhow::bail!("sem captura de tela: o host do Mac precisaria de ScreenCaptureKit cru");
    }
    println!("  entrada avfoundation: existe");
    println!();
    println!("  => host do Mac sai SEM FFI novo: avfoundation + videotoolbox");

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("este probe e do macOS: ele mede as pecas do host de tela de la");
}
