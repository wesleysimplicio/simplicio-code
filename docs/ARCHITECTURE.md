# Arquitetura do Simplicio Code

## Decisão de produto

O usuário assina o **Simplicio**, não provedores individuais. O cliente conhece
somente identidade, entitlement e gateway Simplicio. Chaves e assinaturas de
Grok, OpenRouter, OpenCode Go/Zen ou outros provedores pertencem à infraestrutura
privada do Simplicio e nunca são configuradas ou entregues ao cliente.

## Runtime e Code como um produto

```text
TUI / headless / ACP
        |
        v
AsyncFileSystem (SimplicioRuntimeFs)
        |
        v
MCP stdio: simplicio serve --mcp --stdio --json
        |
        v
simplicio_file_read (sandbox + limite + contrato tipado)
```

O Runtime é a autoridade de leitura, escrita e exclusão, inclusive quando um
cliente ACP anuncia filesystem próprio. `SimplicioRuntimeFs` não possui
fallback local para nenhuma das três: falha de instalação, identidade,
protocolo, capability ausente, sandbox (incluindo escape via symlink) ou
truncamento interrompe a operação. O cliente mantém uma sessão MCP por
workspace, negocia capabilities via `initialize` + `tools/list` e reinicia a
conexão após falha recuperável.

`simplicio-runtime-client` já expõe contratos tipados para `search`, `list`,
`stat`, `edit` (com plano atômico/rollback) e `exec` (argv direto, sem shell,
com bloqueio de metacaracteres) além de `read`/`write`/`delete`, cada um
verificando a capability do Runtime antes de enviar a requisição. `read`,
`write`, `delete` e `search` estão ligados a `SimplicioRuntimeFs` (usado por
TUI/headless/workspace/ACP), que agora implementa também `AsyncSearch` — o
mesmo padrão fail-closed dos três primeiros: Runtime ausente, incompatível ou
um escopo de busca (`path`) que escape do workspace (mesma checagem
canonicalize-based de symlink usada por `relative_path`) bloqueia a operação
com erro acionável, sem fallback local. `search` também valida cada `glob`
contra path traversal/absolute antes de enviar ao Runtime
(`secure_glob`/`Error::GlobRejected`), fechando a mesma classe de bypass que a
correção de symlink do PR anterior fechou para os alvos de leitura/escrita.
Um único `SearchBackend` (recurso em `Resources`, ao lado de `FileSystem`) é
injetado apenas nos dois pontos de construção que já usam
`SimplicioRuntimeFs` — nenhum tool passa a exigi-lo: ausente, o tool mantém
seu comportamento local (ripgrep) inalterado; presente, o tool usa o backend
exclusivamente e falha fechado nos erros dele. `CodexGrepFilesTool`
(`grep_files`) é o primeiro consumidor real. `list`/`stat`/`edit`/`exec`
continuam sem consumidor no agente — essa é a próxima fatia.

O Runtime é um processo acoplado ao binário na experiência do usuário, mas
continua sendo um componente independente e testável. Isso evita duplicar mapa,
memória, busca, action gate e políticas de contexto dentro da TUI.

## Gateway Simplicio

Contrato planejado para a próxima fatia executável:

1. `simplicio-code login` abre device authorization no domínio Simplicio.
2. O cliente recebe token curto e refresh token no keychain do sistema.
3. `GET /v1/code/models` retorna apenas modelos/profiles Simplicio, sem revelar
   credenciais ou contratos upstream.
4. `POST /v1/code/responses` recebe a requisição normalizada e faz roteamento
   privado por qualidade, custo, disponibilidade e política do plano.
5. Entitlement, rate limit, auditoria e medição pertencem ao gateway.

Não haverá BYOK nem seleção pública de assinatura upstream no Simplicio Code.

## Migração incremental

- Concluído: leitura, escrita e exclusão de arquivos do agente via Runtime
  (`SimplicioRuntimeFs`), com sandbox rígido (path traversal + escape via
  symlink) e sem fallback local.
- Concluído: `search` ligado via `AsyncSearch`/`SimplicioRuntimeFs` e
  consumido por `grep_files` (namespace Codex) através do recurso opcional
  `SearchBackend`, com o mesmo sandbox fail-closed (path/glob traversal,
  escape via symlink) e sem fallback local quando o backend está presente.
- Concluído no cliente, pendente de ligação: contratos MCP tipados de
  `list`/`stat`/`edit`/`exec` com capability negotiation e rejeição
  fail-closed de Runtime incompatível — nenhum tool do agente os consome
  ainda.
- Próximo: ligar `grep`/`hashline_grep`/`list_dir` (que hoje chamam
  `ripgrep`/`tokio::fs`/`ignore::WalkBuilder` diretamente, fora de
  `AsyncSearch`/`AsyncFileSystem`) ao mesmo `SearchBackend`/contrato de
  `list`, e o executor de `bash` ao contrato de `exec`.
- Depois: identidade/entitlement Simplicio e gateway único de inferência.
- Por último: instaladores assinados, atualização automática e release privada.

O remoto Git `upstream` continuará apontando para o Simplicio Code para permitir
absorver correções de segurança e compatibilidade sem perder a camada Simplicio.
