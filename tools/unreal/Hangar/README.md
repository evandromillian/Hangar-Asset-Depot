# Hangar para Unreal Engine

Plugin de editor (UE 5.8) que recebe assets enviados pelo Hangar com **Enviar para Unreal** e importa
malha, texturas e materiais no projeto aberto. O editor mantém uma conexão de saída com o servidor
(server-sent events); nada escuta portas na sua máquina, então funciona igual com o Hangar local ou na
nuvem.

## Instalação

1. Copie a pasta `tools/unreal/Hangar` para `Plugins/Hangar` do seu projeto (projetos C++ compilam o
   plugin junto; para projetos só Blueprint, use o pacote gerado por `RunUAT BuildPlugin`, abaixo).
2. Abra o projeto. O plugin **Interchange** precisa estar ativo (vem ativo por padrão).
3. Em **Editor Preferences → Plugins → Hangar**, ajuste o **Servidor** (padrão `http://localhost:8100`)
   e a **Pasta de destino** (padrão `/Game/Hangar`).

## Conectar

1. Na barra de ferramentas do Level Editor, clique em **Hangar: desconectado → Conectar ao Hangar…**.
2. O navegador abre a página **Dispositivos** do Hangar com um código (ex.: `KXRT-4QPM`). Confira que é o
   mesmo da notificação no Unreal e aprove a conexão.
3. O botão passa a mostrar **Hangar: <seu nome>**. O token fica em
   `%LOCALAPPDATA%\Hangar\credentials.json` (um por servidor, compartilhado entre projetos);
   o editor reconecta sozinho ao abrir.

**Desconectar** revoga o acesso deste editor no Hangar e apaga o token deste computador. A página
**Dispositivos** do Hangar lista os editores conectados e também revoga qualquer um deles.

## Enviar um asset

Na página do asset, no Hangar, clique em **Enviar para Unreal** e escolha o editor. O Unreal mostra o
progresso numa notificação e a página do Hangar acompanha o envio. O resultado fica em:

| O quê | Onde |
|---|---|
| Malha (`SM_<asset>` ou `SK_<asset>`) | `<destino>/<projeto>/<asset>/` |
| Texturas do próprio asset | `<destino>/<projeto>/<asset>/Textures/…` |
| Texturas de um conjunto de texturas (`@conjunto/…`) | `<destino>/<projeto>/TextureSets/<conjunto>/…`, importadas uma vez e reutilizadas |
| Materiais (`MI_<asset>_<material>`) | `<destino>/<projeto>/<asset>/Materials/` |
| Skins (variantes) | `<destino>/<projeto>/<asset>/Materials/Variants/` (não atribuídas) |
| Materiais mestres (opaco, mascarado, translúcido) | `<destino>/_Materials/` |
| Arquivos baixados | `Saved/Hangar/` do projeto (cache por hash em `Saved/Hangar/blobs`) |

Enviar a mesma versão de novo atualiza os assets no lugar (referências em níveis e Blueprints continuam
valendo); texturas iguais (mesmo hash) não são reimportadas.

## Limitações desta versão

- Animações (clipes do FBX e pacotes de animação) e áudio ainda não são importados.
- LODs em arquivos separados (`_LOD1`…) ainda não são adicionados à malha; LODs dentro do próprio FBX sim.
- Malhas com skin criam um Skeleton novo.

## Desenvolvimento

- Build standalone:
  `RunUAT.bat BuildPlugin -Plugin=<abs>\Hangar.uplugin -Package=<pasta de saída> -TargetPlatforms=Win64`
- Código em `Source/HangarEditor/Private`:
  - `HangarBridgeSubsystem` — pairing (device flow), `GET /api/bridge/events`, reconexão, fila de imports.
  - `HangarImporter` — `GET /api/versions/{id}/bundle`, downloads por hash (`/api/blobs/{hash}`,
    BLAKE3 verificado), import via Interchange (materiais e texturas do FBX desligados) e relatório em
    `POST /api/bridge/deliveries/{id}`.
  - `HangarMaterials` — materiais mestres gerados em código e as instâncias por material do manifest,
    com as mesmas regras de fator do GLB do Hangar (ORM = AO/R, roughness/G, metallic/B).
