//! Idade do ultimo input na sessao interativa do Windows.
//!
//! E o sinal do dead-man's switch (spec 005): depois de receber o
//! dispositivo, se nenhum input chegar em N segundos, o agente devolve o
//! canal por conta propria -- porque a maquina de ORIGEM nao pode desfazer.
//!
//! GetLastInputInfo e por SESSAO: um servico na sessao 0 nao ve o input do
//! usuario. Por isso o guard tem que rodar na sessao interativa (atalho em
//! shell:startup), nao como servico.

#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetTickCount64() -> u64;
}

/// Milissegundos desde o ultimo input (teclado ou mouse) nesta sessao.
pub fn idle_ms() -> Result<u64, String> {
    let mut lii = LastInputInfo {
        cb_size: std::mem::size_of::<LastInputInfo>() as u32,
        dw_time: 0,
    };
    let ok = unsafe { GetLastInputInfo(&mut lii) };
    if ok == 0 {
        return Err("GetLastInputInfo falhou".into());
    }
    let now = unsafe { GetTickCount64() };
    // dw_time e 32-bit e da volta a cada ~49 dias; GetTickCount64 nao.
    // Truncar o agora para 32 bits e subtrair com wrapping da a diferenca
    // correta mesmo na virada.
    Ok((now as u32).wrapping_sub(lii.dw_time) as u64)
}
