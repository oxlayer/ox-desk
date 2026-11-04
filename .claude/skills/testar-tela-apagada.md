# Testar o host de tela com a tela apagada

Regressão que já quebrou: com a saída de vídeo desligada por economia
de energia, o host subia, aceitava cliente e **nunca mandava quadro**.
Ver [[../rules/tela-bloqueada-windows]].

O caso só aparece quando a máquina ficou sozinha um tempo — que é
justamente quando você quer alcançá-la. Por isso vale forçar.

## 1. Instalar a tarefa que apaga a tela

Tem de rodar **na sessão interativa**: pelo SSH cai na sessão 0, onde
não há desktop para apagar. Daí a tarefa com `/it`.

```powershell
# apagar-tela.ps1, em %LOCALAPPDATA%\logid\
$src = @"
using System;
using System.Runtime.InteropServices;
public class Tela {
  [DllImport("user32.dll")]
  public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
}
"@
Add-Type -TypeDefinition $src
# HWND_BROADCAST=0xffff, WM_SYSCOMMAND=0x0112, SC_MONITORPOWER=0xF170, 2=desligar
[Tela]::SendMessage([IntPtr]0xffff, 0x0112, [IntPtr]0xF170, [IntPtr]2) | Out-Null
```

```powershell
schtasks /create /tn apagar-tela /tr "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$ps`"" /sc once /st 00:00 /it /f
```

## 2. Rodar o teste

```bash
ssh windows-host 'taskkill /F /IM logistream-host.exe'      # NAO Stop-Process
sleep 2
ssh windows-host 'powershell -NoProfile -Command "schtasks /run /tn apagar-tela | Out-Null"'
sleep 25                                              # tempo de verdade apagada
ssh windows-host 'powershell -NoProfile -Command "schtasks /run /tn logistream-host | Out-Null"'
sleep 4
cargo run --release --bin logistream-sink -- windows-host.local:9777 14
```

## 3. O que é passar

O `sink` tem de reportar quadros (dezenas em 14 s). E no log do host,
`enviando quadro 0 CHAVE` logo após `NVENC pronto` — **sem** nenhuma
linha `DuplicateOutput recusou` e **sem** `nenhum quadro em Ns`.

Se aparecer `nenhum quadro em 3s; acordando a tela`, o vigia salvou a
sessão mas o despertar da partida falhou — vale investigar, porque o
vigia é rede de segurança, não o mecanismo.

## 4. Limpar

Deixar uma tarefa que apaga a tela na máquina de alguém confunde depois:

```powershell
schtasks /delete /tn apagar-tela /f
Remove-Item (Join-Path $env:LOCALAPPDATA 'logid\apagar-tela.ps1')
```
