// Onde esta o ponteiro, em pixels. Uma linha por leitura.
//
// Existe porque "parece que nao mexeu" nao e medida. Com a posicao
// lida antes e depois de uma rajada de MouseRel, ou andou os pixels
// esperados ou nao andou -- e a diferenca aponta para o lado certo
// (chegou e foi recusado vs. nem chegou).
//
// Nao precisa de permissao: ler a posicao do cursor nao e TCC.
import CoreGraphics
let p = CGEvent(source: nil)!.location
print("\(Int(p.x)) \(Int(p.y))")
