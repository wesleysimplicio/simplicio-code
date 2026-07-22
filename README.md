<div align="center">

<h1>Simplicio Code (<code>simplicio-code</code>)</h1>

**Simplicio Code** é o agente de programação da assinatura Simplicio. Ele une
uma interface terminal/ACP em Rust ao Simplicio Runtime: toda leitura de arquivo
do projeto feita pelo agente passa pelo contrato MCP do Runtime, com sandbox,
controle de contexto e economia de tokens.

[Installing the released binary](#instalando-o-binário) ·
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

Versão atual: **0.3.0-beta.2**.

- leitura de arquivos obrigatoriamente via `simplicio_file_read`;
- handshake MCP valida que o processo é o Simplicio Runtime verdadeiro;
- falha fechada: sem Runtime, o agente não lê diretamente do disco;
- TUI, headless, workspace e ACP compartilham o mesmo backend de leitura;
- ao abrir uma pasta, o Runtime inicia o mapa geral em segundo plano;
- o modelo aparece como **Simplicio-1** e usa `tencent/hy3:free` via OpenRouter;
- o tema padrão **Simplicio Brasil** usa verde e amarelo;
- escrita/exclusão usam o Runtime; `apply_patch` envia o plano completo pelo
  contrato atômico `simplicio_edit`, sem fallback local em sessões produtivas.

Para desenvolvimento local, forneça a credencial apenas pelo ambiente:

```sh
export OPENROUTER_API_KEY="..."
```

A chave nunca deve ser gravada no repositório nem distribuída no binário. A
sincronização de login e assinatura Simplicio será adicionada numa atualização
posterior ao beta, quando o cliente passará a consumir o gateway autenticado.

Veja [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) para as fronteiras do produto.

## Instalando o binário

A partir da `0.3.0-beta.1`, releases assinadas (prerelease no GitHub) passam
a ser publicadas por [`.github/workflows/release.yml`](.github/workflows/release.yml),
com checksum, SBOM e manifest assinado — veja
[`RELEASE_NOTES_0.3.0-beta.2.md`](RELEASE_NOTES_0.3.0-beta.2.md) para o que
já funciona e o que ainda falta (chave de assinatura de produção, build
Windows, rollout gradual). Instale com:

```sh
curl -fsSL https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.sh | bash
```

```powershell
irm https://raw.githubusercontent.com/wesleysimplicio/simplicio-code/main/install.ps1 | iex
```

Ambos os scripts baixam apenas releases publicadas por **este** repositório
(não a infraestrutura x.ai herdada em `crates/codegen/xai-grok-pager/scripts/`)
e recusam a instalação se o checksum não bater.

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
- **ripgrep (`rg`)** — release builds of `xai-grok-tools` and `xai-grok-shell`
  bundle a static `rg` binary for the in-app search/shell tools. Resolution
  order: (1) an explicit override env var always wins if set —
  `GROK_TOOLS_BUNDLE_RG_PATH` for `xai-grok-tools`, `GROK_SHELL_BUNDLE_RG_PATH`
  for `xai-grok-shell` — pointing at a local `rg` binary to bundle; (2) an
  `rg` already on `PATH` is detected automatically and bundled, no network
  access needed; (3) otherwise the build script downloads a pinned `rg`
  release from GitHub Releases, which requires outbound network access and
  fails on egress-restricted hosts/proxies. On such hosts, either install
  `ripgrep` so it's on `PATH` before building, or set the override env var.
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

Simplicio-specific docs (start here for this fork):

- [docs/QUICKSTART.md](docs/QUICKSTART.md) — install and first run, PT + EN
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — client, Runtime, and gateway boundaries
- [docs/audits/issue-139-report.md](docs/audits/issue-139-report.md) — reproducible issue specification audit and closure decisions; its [hash-guarded rewrite bundle](docs/audits/issue-139-rewrites.json) provides owner-review drafts without mutating or closing issues
- [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) — login, Runtime, mapa, rede, updater
- [docs/privacy/telemetry.md](docs/privacy/telemetry.md) — what telemetry exists, opt-out, `privacy diagnose`
- [docs/privacy/network-destinations.md](docs/privacy/network-destinations.md) — every network destination the client can contact, the telemetry-scoped allowlist, and the network-capture test
- [docs/migration/legacy-login-migration.md](docs/migration/legacy-login-migration.md) — design for the future login/entitlement migration (pending #3/#4)

Run `python3 scripts/check_doc_links.py` to validate every doc link and
referenced `cargo -p <crate>` command below and under `docs/`.

Full online documentation for the underlying CLI is available at
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
