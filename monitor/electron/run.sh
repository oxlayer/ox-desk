#!/usr/bin/env bash
# Sobe o app de desktop. A limpeza de ELECTRON_RUN_AS_NODE e obrigatoria
# quando o terminal vem de dentro do VS Code (o host de extensoes a exporta,
# e com ela o binario do Electron roda como Node puro e nada abre).
cd "$(dirname "$0")"
exec env -u ELECTRON_RUN_AS_NODE ./node_modules/.bin/electron . "$@"
