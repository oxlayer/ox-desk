# 013 — Tela de bloqueio do Windows (estilo Parsec)

> Estado: **mecanismo provado**, serviço não escrito. Falta a medição
> com a máquina de fato bloqueada.

## O problema

A tela remota do Windows morre quando a sessão bloqueia. O
`capture_win.rs` já documenta as quatro causas do mesmo
`E_ACCESSDENIED` ([regra](../.claude/rules/windows-locked-screen.md)),
e três delas são a mesma coisa por baixo: **a mesa de entrada mudou**.

A tela de bloqueio, o diálogo de UAC e o protetor de tela vivem em
mesas (desktops) próprias dentro da window station `WinSta0`. O DXGI
não duplica mesa que não seja a de entrada, e só `LocalSystem`
consegue se anexar à mesa `Winlogon`.

Isso importa mais agora do que importava ontem: acessando de fora de
casa ([spec 012](012-outside-the-lan.md)), a máquina vai estar bloqueada
quase sempre. Hoje isso é exatamente o estado em que ela não serve
tela.

## O que foi medido

Com o `logistream-probe-sistema`, nesta máquina (Windows 10 Pro,
2560x1440):

**Como usuário, pela sessão SSH:**

```
pid 29568 sessão 0
sessão do console: 1
USERNAME=user (SYSTEM? false)
mesa desta thread: Default
mesa de ENTRADA: não consegui abrir (Função incorreta. 0x80070001)
```

A fronteira de sessão inteira em cinco linhas: o SSH cai na sessão 0, o
console é a 1, e a mesa de entrada nem abre
([regra](../.claude/rules/graphical-session.md)).

**Como SYSTEM, por tarefa `/ru SYSTEM`:**

```
winlogon.exe pid 524
privilégio SeDebugPrivilege: ligado
privilégio SeIncreaseQuotaPrivilege: ligado
privilégio SeAssignPrimaryTokenPrivilege: ligado
LANCEI pid 11656 na mesa winsta0\Winlogon
  → pid 11656 sessão 1
  → USERNAME=windows-host$ (SYSTEM? true)
  → mesa desta thread: Winlogon
  → mesa de ENTRADA: Default
  → anexado à mesa de entrada: Default
  → DUPLICOU: 2560x1440
```

Ou seja: o caminho funciona. Um processo SYSTEM lançado assim **troca
de mesa conforme a entrada** e captura.

## O mecanismo (o mesmo de Parsec e RustDesk)

1. Serviço LocalSystem, sessão 0.
2. `WTSGetActiveConsoleSessionId()` → a sessão interativa.
3. Achar `winlogon.exe` **naquela sessão**. É o alvo porque existe
   sempre, roda como SYSTEM, e **continua existindo sem usuário
   logado** — que é o estado da tela de bloqueio. O `explorer` não
   serve: é do usuário e some.
4. `OpenProcessToken` + `DuplicateTokenEx(..., TokenPrimary, ...)`.
   Primário, não de impersonação: o `CreateProcessAsUser` recusa o
   outro, e a documentação é explícita.
5. `CreateProcessAsUserW` com `lpDesktop = "winsta0\\Winlogon"`.
6. No filho: `OpenInputDesktop` + `SetThreadDesktop` a cada troca, e
   recriar a duplicação do DXGI.

Privilégios: `SeDebugPrivilege` para abrir o winlogon,
`SeIncreaseQuotaPrivilege` e `SeAssignPrimaryTokenPrivilege` para o
`CreateProcessAsUser`. Sem eles a chamada falha com
`ERROR_PRIVILEGE_NOT_HELD`, e a mensagem não menciona privilégio.

Detalhe que custaria um susto: o filho nasce com `CREATE_NO_WINDOW`.
Com `CREATE_NEW_CONSOLE` abriria uma janela preta **na tela de
bloqueio** — bem onde ninguém quer ver uma.

## O que falta medir, e por que não foi medido

**A máquina bloqueada.** Com ela destravada a mesa de entrada é
`Default`, e foi o que o probe capturou. Bloqueada, a mesa vira
`Winlogon`, e aí o comportamento **varia por versão do Windows**: há
relato de o processo morrer logo após o bloqueio no Win 11 e relato de
funcionar como SYSTEM. Também há relato de o DXGI inicializar na
Winlogon e devolver quadros **vazios** sem falhar — daí a necessidade
de queda para GDI.

Não dá para decidir isso por leitura, e o teste exige bloquear a
máquina de verdade — o que custa a senha de volta. Por isso ficou como
pedido explícito, não como algo a fazer sozinho.

```powershell
# bloquear, esperar, rodar como SYSTEM, ler
rundll32 user32.dll,LockWorkStation
schtasks /run /tn ox-probe-sistema
Get-Content C:\ProgramData\logid\probe-sistema.log
```

## Desenho proposto

Um binário novo, não o host atual:

| Hoje | Proposto |
|---|---|
| tarefa `/IT`, roda como o usuário | serviço LocalSystem lança o host na sessão do console |
| captura só a mesa `Default` | segue a mesa de entrada |
| morre no bloqueio | sobrevive |

### A decisão que não é óbvia

Rodar o host **sempre** como SYSTEM é mais privilégio do que ele
precisa 95% do tempo, e esta máquina já teve o `logid.exe` apagado
pelo Defender uma vez (spec 008). Um binário SYSTEM permanente que
captura tela e injeta teclado é exatamente o perfil que um antivírus
odeia — com razão.

Alternativa: manter o host do usuário como principal e o serviço só
assume quando a mesa de entrada deixa de ser `Default`. Custa um
handoff e uma segunda conexão; ganha que o caso comum segue sem
privilégio.

A escolha depende do que a medição com a máquina bloqueada mostrar: se
o processo SYSTEM morre ao bloquear (como um relato diz), o handoff
vira obrigatório de qualquer jeito.

## Por que probe antes de serviço

Instalar serviço mexe em `HKLM` e cria processo SYSTEM permanente.
O probe consegue o mesmo efeito por tarefa `/ru SYSTEM`, que o
administrador cria e apaga sem resíduo — e responde a pergunta que
decide o desenho antes de pedir a mudança maior. Mesmo espírito do
`mac-probe`: provar as peças antes de montar o pipeline.

## Pendências menores

- O log do probe (`C:\ProgramData\logid\probe-sistema.log`) sai em
  UTF-8 e o PowerShell lê como ANSI: acentos chegam trocados. Cosmético
  e não afeta a medição, mas vai confundir alguém.
- `SendSAS` (Ctrl+Alt+Del remoto) não foi tentado. Sem ele não dá para
  destravar a máquina de fora — só ver a tela de bloqueio.

## Veja também

- [[../.claude/rules/windows-locked-screen.md]] — as quatro causas do mesmo erro
- [[../.claude/rules/graphical-session.md]] — a fronteira, medida de novo aqui
- [[012-outside-the-lan]] — por que isto virou prioridade
