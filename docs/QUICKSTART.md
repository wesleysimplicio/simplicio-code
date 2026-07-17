# Quickstart — Simplicio Code

Two languages, one doc: [Português](#português) first, [English](#english) below.
Both sections cover the same three things — install, first run, and where to
go when something breaks — with no OpenRouter/provider setup required (see
[Estado do produto](../README.md#estado-do-produto)).

---

## Português

### 1. Pré-requisitos

- **Rust** — a versão é fixada por [`rust-toolchain.toml`](../rust-toolchain.toml);
  o `rustup` instala automaticamente na primeira build.
- **[DotSlash](https://dotslash-cli.com)** — necessário para as ferramentas
  hermeticamente empacotadas em [`bin/`](../bin/) (principalmente
  [`bin/protoc`](../bin/protoc)):

  ```sh
  cargo install dotslash
  dotslash --help   # checagem de sanidade
  ```

- **protoc** — a geração de código de proto resolve `bin/protoc` via DotSlash,
  ou usa um `protoc` do `PATH`/`$PROTOC` como alternativa.
- macOS e Linux são os hosts de build suportados; builds no Windows são
  best-effort e não são testadas a partir desta árvore.

### 2. Build e primeira execução

```sh
cargo build -p xai-grok-pager-bin --bin simplicio-code --release
./target/release/simplicio-code --version
```

Abra uma pasta de projeto normalmente:

```sh
cd /caminho/do/seu/projeto
simplicio-code
```

Não é preciso fornecer uma chave de provedor (OpenRouter, xAI, etc.) para
usar o beta: o modelo aparece como **Simplicio-1** e a credencial de
desenvolvimento (`OPENROUTER_API_KEY`), quando necessária localmente, nunca é
solicitada pela interface nem fica registrada no histórico. Veja
[docs/ARCHITECTURE.md](ARCHITECTURE.md) para os limites do produto.

### 3. O que acontece na primeira abertura de pasta

1. O Simplicio Runtime é iniciado como processo MCP acoplado ao binário
   (`simplicio serve --mcp --stdio --json`).
2. Toda leitura de arquivo do agente passa por `simplicio_file_read` —
   **fail-closed**: se o Runtime não responder ao handshake, o agente não lê
   diretamente do disco.
3. O Runtime começa, em segundo plano, o mapa geral do projeto (Mapper).

Consulte [docs/TROUBLESHOOTING.md](TROUBLESHOOTING.md) se qualquer uma dessas
etapas falhar.

### 4. Privacidade e telemetria

- Telemetria é **desabilitada por padrão**. Para conferir exatamente o que
  seria enviado, sem enviar nada:

  ```sh
  simplicio-code privacy diagnose
  ```

- Para desativar de forma explícita e definitiva (convenção
  [`DO_NOT_TRACK`](https://consoledonottrack.com/), reconhecida por qualquer
  ferramenta que respeite o padrão, não só o Simplicio Code):

  ```sh
  export DO_NOT_TRACK=1
  ```

- Dentro de uma sessão, `/privacy` mostra o status atual e `/privacy
  opt-in`/`/privacy opt-out` alterna o compartilhamento de dados de código.

### 5. Limitações conhecidas do beta

- Escrita e exclusão de arquivos ainda usam o backend local existente (só a
  leitura passa pelo Runtime nesta fatia).
- Sincronização de login/assinatura Simplicio ainda não está disponível —
  veja [docs/migration/legacy-login-migration.md](migration/legacy-login-migration.md)
  para o desenho da migração planejada.
- Instaladores públicos assinados ainda não foram publicados.

---

## English

### 1. Prerequisites

- **Rust** — pinned by [`rust-toolchain.toml`](../rust-toolchain.toml);
  `rustup` installs it automatically on first build.
- **[DotSlash](https://dotslash-cli.com)** — required so the hermetic tools
  under [`bin/`](../bin/) (notably [`bin/protoc`](../bin/protoc)) can
  download and run:

  ```sh
  cargo install dotslash
  dotslash --help   # sanity check
  ```

- **protoc** — proto codegen resolves `bin/protoc` via DotSlash, or falls
  back to a `protoc` on `PATH`/`$PROTOC`.
- macOS and Linux are supported build hosts; Windows builds are best-effort
  and not currently tested from this tree.

### 2. Build and first run

```sh
cargo build -p xai-grok-pager-bin --bin simplicio-code --release
./target/release/simplicio-code --version
```

Open a project folder as usual:

```sh
cd /path/to/your/project
simplicio-code
```

No provider key (OpenRouter, xAI, etc.) is required to use the beta: the
model shows up as **Simplicio-1**, and the local-dev credential
(`OPENROUTER_API_KEY`), when needed, is never requested by the UI and never
logged. See [docs/ARCHITECTURE.md](ARCHITECTURE.md) for product boundaries.

### 3. What happens the first time you open a folder

1. The Simplicio Runtime starts as an MCP process coupled to the binary
   (`simplicio serve --mcp --stdio --json`).
2. Every agent file read goes through `simplicio_file_read` —
   **fail-closed**: if the Runtime doesn't answer the handshake, the agent
   does not fall back to reading the disk directly.
3. The Runtime kicks off the project-wide map (Mapper) in the background.

See [docs/TROUBLESHOOTING.md](TROUBLESHOOTING.md) if any of these steps
fail.

### 4. Privacy and telemetry

- Telemetry is **disabled by default**. To see exactly what would be sent,
  without sending anything:

  ```sh
  simplicio-code privacy diagnose
  ```

- To opt out explicitly and permanently (the
  [`DO_NOT_TRACK`](https://consoledonottrack.com/) community convention,
  honored by any tool that respects it — not a Simplicio-specific flag):

  ```sh
  export DO_NOT_TRACK=1
  ```

- Inside a session, `/privacy` shows current status, and `/privacy
  opt-in`/`/privacy opt-out` toggles coding-data sharing.

### 5. Known beta limitations

- Write and delete operations still use the existing local backend (only
  reads go through the Runtime in this slice).
- Simplicio login/subscription sync is not available yet — see
  [docs/migration/legacy-login-migration.md](migration/legacy-login-migration.md)
  for the planned migration design.
- Signed public installers have not been published yet.
