// A tecla A esta apertada? Observa por 4s e responde.
//
// O Caps Lock NAO serve de teste no macOS: o sistema o trata na camada
// HID e um evento sintetico nao o alterna, mesmo chegando e sendo
// traduzido. Ja mediu "nao funciona" com o caminho inteiro funcionando.
// Uma tecla comum, olhada enquanto esta segurada, e medida de verdade.
//
// kVK_ANSI_A = 0, que e o codigo do macOS -- o protocolo carrega evdev,
// onde A = 30. Tres numeracoes convivem neste projeto; ver
// `tecla_mac_de_evdev` em logistream/src/input.rs.
import CoreGraphics
import Foundation

var vista = false
for _ in 0..<400 {
    if CGEventSource.keyState(.combinedSessionState, key: 0) { vista = true; break }
    usleep(10_000)
}
print(vista ? "A: PRESSIONADA" : "A: nunca apertada")
