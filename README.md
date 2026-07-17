<div align="center">

<h1>Simplicio Code (<code>simplicio-code</code>)</h1>

**Simplicio Code** é o agente de programação da assinatura Simplicio. Ele une
uma interface terminal/ACP em Rust ao Simplicio Runtime: toda leitura de arquivo
do projeto feita pelo agente passa pelo contrato MCP do Runtime, com sandbox,
controle de contexto e economia de tokens.

[Installing the released binary](#installing-the-released-binary) ·
[Building from source](#building-from-source) ·
[Documentation](#documentation) ·
[Repository layout](#repository-layout) ·
[Development](#development) ·
[Contributing](#contributing) ·
[License](#license)

Este é um fork privado de produto. O remoto `upstream` preserva a origem do
Simplicio Code; integrações próprias vivem neste repositório.

A small `SOURCE_REV` file at the root records the full monorepo commit SHA
for the version of the code present in this tree.

</div>

---

## Estado do produto

Versão atual: **0.3.0-beta.1**.

- leitura de arquivos obrigatoriamente via `simplicio_file_read`;
- handshake MCP valida que o processo é o Simplicio Runtime verdadeiro;
- falha fechada: sem Runtime, o agente não lê diretamente do disco;
- TUI, headless, workspace e ACP compartilham o mesmo backend de leitura;
- ao abrir uma pasta, o Runtime inicia o mapa geral em segundo plano;
- o modelo aparece como **Simplicio-1** e usa `tencent/hy3:free` via OpenRouter;
- o tema padrão **Simplicio Brasil** usa verde e amarelo;
- escrita/exclusão ainda usam o backend local existente nesta primeira versão.

Para desenvolvimento local, forneça a credencial apenas pelo ambiente:

```sh
export OPENROUTER_API_KEY="..."
```

A chave nunca deve ser gravada no repositório nem distribuída no binário. A
sincronização de login e assinatura Simplicio será adicionada numa atualização
posterior ao beta, quando o cliente passará a consumir o gateway autenticado.

Veja [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) para as fronteiras do produto.

## Instalando o binário

Os instaladores públicos serão publicados após a primeira release privada:

```sh
simplicio-code --version
```

## Building from source

Requirements:

- **Rust** — the toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml);
  `rustup` installs it automatically on first build.
- **[DotSlash](https://dotslash-cli.com)** — required so hermetic tools under
  [`bin/`](bin/) (notably [`bin/protoc`](bin/protoc)) can download and run.
  Install it and ensure `dotslash` is on your `PATH` **before** building:

  ```sh
  cargo install dotslash
  # or: prebuilt packages — https://dotslash-cli.com/docs/installation/
  /usr/bin/env dotslash --help   # sanity check
  ```

- **protoc** — proto codegen resolves [`bin/protoc`](bin/protoc) via DotSlash,
  or falls back to a `protoc` on `PATH` / `$PROTOC`.
- macOS and Linux are supported build hosts; Windows builds are best-effort
  and not currently tested from this tree.

```sh
cargo run -p xai-grok-pager-bin --bin simplicio-code
cargo build -p xai-grok-pager-bin --bin simplicio-code --release
cargo check -p xai-grok-pager-bin            # fast validation
```

O artefato é `target/release/simplicio-code`. O fluxo de autenticação Simplicio
e o gateway de inferência serão conectados antes da primeira distribuição.

## Documentation

Full online documentation is available at
[docs.x.ai/build/overview](https://docs.x.ai/build/overview).

The user guide ships with the pager crate:
[`crates/codegen/xai-grok-pager/docs/user-guide/`](crates/codegen/xai-grok-pager/docs/user-guide/)
— getting started, keyboard shortcuts, slash commands, configuration, theming,
MCP servers, skills, plugins, hooks, headless mode, sandboxing, and more.

## Repository layout

| Path | Contents |
|------|----------|
| `crates/codegen/xai-grok-pager-bin` | Composition-root package; builds the `xai-grok-pager` binary |
| `crates/codegen/xai-grok-pager` | The TUI: scrollback, prompt, modals, rendering |
| `crates/codegen/xai-grok-shell` | Agent runtime + leader/stdio/headless entry points |
| `crates/codegen/xai-grok-tools` | Tool implementations (terminal, file edit, search, ...) |
| `crates/codegen/xai-grok-workspace` | Host filesystem, VCS, execution, checkpoints |
| `crates/codegen/...` | The rest of the CLI crate closure (config, MCP, markdown, sandbox, ...) |
| `crates/common/`, `crates/build/`, `prod/mc/` | Small shared leaf crates pulled in by the closure |
| `third_party/` | Vendored upstream source (Mermaid diagram stack) — see below |

> [!IMPORTANT]
> The root `Cargo.toml` (workspace members, dependency versions, lints,
> profiles) is **generated** — treat it as read-only. Prefer editing per-crate
> `Cargo.toml` files.

## Development

```sh
cargo check -p <crate>        # always target specific crates; full-workspace builds are slow
cargo test -p xai-grok-config # per-crate tests
cargo clippy -p <crate>       # lint config: clippy.toml at the repo root
cargo fmt --all               # rustfmt.toml at the repo root
```

## Contributing

> [!NOTE]
> External contributions are not accepted. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

First-party code in this repository is licensed under the **Apache License,
Version 2.0** — see [`LICENSE`](LICENSE).

Third-party and vendored code remains under its original licenses. See:

- [`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES) — crates.io / git dependencies,
  bundled UI themes, and **in-tree source ports** (including openai/codex and
  sst/opencode tool implementations)
- [`crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md`](crates/codegen/xai-grok-tools/THIRD_PARTY_NOTICES.md)
  — crate-local notice for the codex and opencode ports (license texts +
  Apache §4(b) change notice)
- [`third_party/NOTICE`](third_party/NOTICE) — vendored Mermaid-stack index
