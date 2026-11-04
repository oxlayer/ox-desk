//! Novidades, embutidas no binario.
//!
//! `include_str!` e nao arquivo em disco de proposito: o que o painel
//! mostra tem de ser o que AQUELA maquina esta rodando. Lendo de disco,
//! uma maquina com binario velho mostraria novidades que ela nao tem --
//! que e exatamente o engano que o numero de build existe para evitar.

use serde::Serialize;

const TEXTO: &str = include_str!("../CHANGELOG.md");

#[derive(Debug, Serialize, PartialEq)]
pub struct Entrada {
    pub data: String,
    pub titulo: String,
    pub descricao: String,
}

/// Lê `## <data> · <titulo>` seguido das linhas de descricao.
pub fn entradas() -> Vec<Entrada> {
    let mut v = Vec::new();
    let mut atual: Option<Entrada> = None;
    for linha in TEXTO.lines() {
        if let Some(cab) = linha.strip_prefix("## ") {
            if let Some(e) = atual.take() {
                v.push(e);
            }
            // O separador e `·`; sem ele, tudo vira titulo e a data fica
            // vazia -- melhor que descartar a entrada.
            let (data, titulo) = match cab.split_once('·') {
                Some((d, t)) => (d.trim().to_string(), t.trim().to_string()),
                None => (String::new(), cab.trim().to_string()),
            };
            atual = Some(Entrada { data, titulo, descricao: String::new() });
        } else if let Some(e) = atual.as_mut() {
            let t = linha.trim();
            if t.is_empty() {
                continue;
            }
            if !e.descricao.is_empty() {
                e.descricao.push(' ');
            }
            e.descricao.push_str(t);
        }
    }
    if let Some(e) = atual {
        v.push(e);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_arquivo_de_verdade_parseia() {
        let v = entradas();
        assert!(v.len() >= 5, "poucas entradas: {}", v.len());
        // Se o formato quebrar, o painel mostraria titulos vazios em
        // silencio -- este assert e o que torna isso barulhento.
        for e in &v {
            assert!(!e.titulo.is_empty(), "entrada sem titulo: {e:?}");
            assert!(!e.descricao.is_empty(), "entrada sem descricao: {e:?}");
            assert!(e.data.starts_with("20"), "data estranha: {e:?}");
        }
    }

    #[test]
    fn descricao_de_varias_linhas_vira_uma() {
        // O markdown quebra linha por largura; a UI quer um paragrafo.
        let v = entradas();
        let multi = v.iter().find(|e| e.descricao.len() > 80);
        assert!(multi.is_some(), "esperava ao menos uma descricao longa");
        assert!(!multi.unwrap().descricao.contains('\n'));
    }
}
