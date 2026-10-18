# Hangar

Gerenciador de ativos digitais (DAM) para 3D e games — modelos, texturas, áudio — com
**versionamento**, **visualização 3D no navegador** (LODs e variantes de textura) e um
**pipeline de conversão** (FBX/glTF/OBJ → GLB). Escrito inteiramente em Rust:

* **backend**: [summer-rs](https://github.com/summer-rs/summer-rs) 0.7 (axum, SeaORM 2, OpenDAL, sa-token)
* **frontend**: [Leptos](https://leptos.dev) 0.8 (CSR/wasm) + [Bevy](https://bevyengine.org) 0.19 embutido via `leptos-bevy-canvas`
* **pipeline**: `ufbx`, `gltf`, `tobj`, `meshopt`, `image` + fallback opcional no Blender headless

Arquitetura, modelo de dados e API: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Como funciona o versionamento

Cada asset (ex.: *M1A2 Abrams*) tem versões imutáveis `v1, v2, …`. Uma versão é uma árvore
`caminho → blob` (BLAKE3) com um `parent`. Ao criar uma versão a partir de outra, a árvore do
pai é herdada: para corrigir o casco basta enviar o `.fbx` novo — as texturas de 4K continuam
apontando para os mesmos blobs, sem upload e sem ocupar disco de novo. O navegador calcula
os hashes localmente e só envia o que o servidor ainda não tem.

Cada versão é processada em segundo plano: GLB por LOD (autorais `_LOD1`… ou gerados),
detecção de variantes de textura (pastas `Desert/`, `Woodland/` ou tokens `T_Body_Desert_D`),
correção de caminhos de textura quebrados do FBX, thumbnail e estatísticas.

## Requisitos

* Rust estável (1.89+) com o target wasm: `rustup target add wasm32-unknown-unknown`
* [Trunk](https://trunkrs.dev): `cargo install trunk --locked` (ou binário do GitHub)
* PostgreSQL 14+ (o `docker-compose.yml` sobe um)
* Opcional: Blender 3.6+/4.x no `PATH` para importar `.blend`, `.dae`, `.3ds`, `.stl`, `.ply`
  e como fallback quando o importador nativo falha

## Rodando

```bash
# 1. banco
docker compose up -d

# 2. frontend (gera crates/hangar-web/dist, servido pelo backend)
cd crates/hangar-web && trunk build --release && cd ../..

# 3. backend (a partir da raiz do repositório; lê config/app.toml)
cargo run -p hangar-server --release
```

Abra <http://localhost:8100>. O primeiro usuário cadastrado vira administrador.

Desenvolvimento com hot-reload do frontend: deixe o backend rodando e use
`cd crates/hangar-web && trunk serve` → <http://localhost:8080> (proxy de `/api` para a porta 8100).

### Configuração

`config/app.toml` (variáveis de ambiente entre `${...}`):

| variável | padrão | uso |
|---|---|---|
| `DATABASE_URL` | `postgres://hangar:hangar@localhost:5432/hangar` | Postgres |
| `HANGAR_DATA` | `./data` | raiz do storage local (blobs) |
| `HANGAR_WEB_DIST` | `crates/hangar-web/dist` | frontend compilado |
| `HANGAR_BLENDER` | `auto` | caminho do Blender, `auto` (PATH) ou vazio para desativar |
| `HANGAR_BIND` | `0.0.0.0` | interface HTTP (porta 8100) |

Para S3/MinIO troque `[opendal] scheme = "s3"` e as opções (exemplo no arquivo; `docker compose --profile s3 up -d` sobe um MinIO).
Sessões ficam em memória por padrão; compile com `--features redis-sessions` e adicione a seção `[redis]` para persistir.

## Estrutura

```
crates/
  hangar-core/      DTOs e tipos compartilhados (server + wasm)
  hangar-pipeline/  importação, LODs, variantes, GLB, thumbnails
  hangar-server/    API summer-rs, storage, jobs, migrations SQL
  hangar-web/       UI Leptos + viewer Bevy
config/app.toml     configuração do servidor
tools/blender/      script de exportação headless
docs/               arquitetura
```

## Testes

```bash
cargo test --workspace --exclude hangar-web
```
