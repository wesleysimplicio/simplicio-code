# Arquitetura do Simplicio Code

## Decisão de produto

O usuário assina o **Simplicio**, não provedores individuais. O cliente conhece
somente identidade, entitlement e gateway Simplicio. Chaves e assinaturas de
Grok, OpenRouter, OpenCode Go/Zen ou outros provedores pertencem à infraestrutura
privada do Simplicio e nunca são configuradas ou entregues ao cliente.

## Agent, Runtime e Code como um produto

O mapeamento auditável entre cada comando/capability e suas superfícies fica em
[`capability-command-inventory.v1.json`](capability-command-inventory.v1.json).
Um atalho genérico não é evidência de cobertura: uma entrada só deixa
`adapter-required` depois de ter boundary tipado, policy, receipt e teste de
contrato na superfície indicada.

```text
TUI / headless / ACP
        |
        v
AgentHost v1 obrigatório (status + advisories)
        |
        v
AsyncFileSystem / AsyncSearch (SimplicioRuntimeFs)
        |
        v
MCP stdio: simplicio serve --mcp --stdio --json
        |
        v
Runtime tools (sandbox + limites + contratos tipados)
```

Simplicio Agent e Simplicio Runtime continuam produtos independentes: nenhum
deles importa ou depende do Code. A dependência é unidirecional. O Code valida
primeiro o AgentHost (`simplicio.agent-host/v1`, `agent/v1`, capabilities
obrigatórias), depois inicia/valida o Runtime. Ausência, versão incompatível ou
socket inseguro de qualquer um bloqueia a operação, sem agente embutido e sem
fallback local. O canal `host.advisories` é passivo, limitado e livre de
conteúdo arbitrário; ele alimenta o modelo de atenção da futura lateral sem
criar outro coordinator ou scheduler dentro do Code.

Em produção, o Agent é o único coordinator cognitivo; os modos `builtin` e
`external` ficam restritos a diagnóstico/compatibilidade isolados. Diagnóstico
pode relatar que Agent ou Runtime está ausente, mas nenhum turno produtivo ou
efeito de projeto pode continuar nessa condição.

O Runtime é a autoridade de leitura, escrita e exclusão, inclusive quando um
cliente ACP anuncia filesystem próprio. `SimplicioRuntimeFs` não possui
fallback local para nenhuma das três: falha de instalação, identidade,
protocolo, capability ausente, sandbox (incluindo escape via symlink) ou
truncamento interrompe a operação. `SharedRuntimeClient` mantém um slot de
sessão MCP compartilhado por workspace dentro do processo, negocia capabilities
via `initialize` + `tools/list` uma vez e reinicia a conexão compartilhada após
falha recuperável.

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
(`grep_files`) continua usando esse backend exclusivamente. O `apply_patch`
do Codex usa a mesma `FileSystem` resource: ele calcula o plano somente com
leituras do backend e, quando o backend oferece edição atômica, envia um único
plano ao contrato Runtime `simplicio_edit`. `SimplicioRuntimeFs` implementa
essa capacidade chamando seu `edit_workspace`, portanto approval/checkpoint/
rollback/receipt continuam sob a autoridade do Runtime. Um erro do Runtime é
terminal para o patch; não há fallback para `write_file`/`delete_file` locais.
Backends locais usados apenas por fixtures e superfícies legadas continuam
com o aplicador por arquivo já existente. `list`/`stat`/`exec` permanecem sem
consumidor produtivo nesta fatia.

O Runtime é um processo acoplado ao binário na experiência do usuário, mas
continua sendo um componente independente e testável. Isso evita duplicar mapa,
memória, busca, action gate e políticas de contexto dentro da TUI.

Para goals interativos que exigem waves, múltiplas issues ou coordenação
paralela, `simplicio-runtime-client::loop_hub::LoopHubClient` é o adapter
prioritário: ele reutiliza a sessão negociada e encaminha a admissão para a
fila única do Loop Hub com prioridade `interactive`, backpressure, cursor de
progress, cancel, resume e receipts idempotentes. O adapter valida que Runtime,
Mapper, scheduler e inference têm um único dono Loop Hub, que a capacidade de
inferência ativa é um único slot e que não existe scheduler local. Code não
cria processo, worker ou fila de recursos nesses caminhos.

O daemon Loop Hub e a autoridade de fila/mapa/runtime continuam sendo
dependências externas. O Code agora fornece também o adapter
`SocketPipeHubTransportFactory`, que abre somente um Unix socket ou named pipe
já existente, negocia `handshake`/`attach` versionados e reconecta com cursores
de progress. `required` falha fechado sem endpoint, handshake ou attach
compatível; submit/cancel/resume não são repetidos quando o receipt fica
desconhecido. O modo `standalone` só existe quando selecionado explicitamente e
não é fallback silencioso.

O recorte atual torna Agent + Runtime obrigatórios nos pontos que já usam
`SimplicioRuntimeFs` (TUI/headless/workspace/ACP) e entrega o contrato tipado de
advisories. Ainda não significa que todos os comandos herdados do Code passam
pelos dois componentes: `list`/`stat`/`exec`, alguns caminhos diretos de
`ripgrep`/`tokio::fs` e a renderização da lateral continuam nas próximas
fatias. Essa fronteira é registrada explicitamente para não confundir contrato
P0 real com integração total ainda não entregue.

Também permanece pendente o contrato neutro de proatividade real:
`workspace.observe` + `workspace.advisory` com finding/risk/suggestion,
privacidade e aprovação. Os eventos atuais são apenas saúde/backpressure/
resultado do host; não observam o que o desenvolvedor está fazendo e não devem
ser apresentados como a lateral proativa completa.

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

- Concluído: cliente AgentHost fail-closed com handshake/versionamento,
  capabilities obrigatórias, socket Unix privado e replay bounded de eventos
  operacionais; `SimplicioRuntimeFs` exige Agent antes do Runtime.
- Concluído: `AgentAttentionState` para a lateral passiva sem roubar foco,
  executar efeitos ou duplicar o scheduler; e `/simplicio <instrução>` como
  entrada explícita que executa um turno no AgentHost e devolve o resultado ao
  scrollback. No Windows, o cliente usa loopback autenticado quando AF_UNIX
  não está disponível.
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
- Próximo: renderizar/pollear a lateral e ligar
  `grep`/`hashline_grep`/`list_dir` (que hoje chamam
  `ripgrep`/`tokio::fs`/`ignore::WalkBuilder` diretamente, fora de
  `AsyncSearch`/`AsyncFileSystem`) ao mesmo `SearchBackend`/contrato de
  `list`, e o executor de `bash` ao contrato de `exec`.
- Depois: identidade/entitlement Simplicio e gateway único de inferência.
- Por último: instaladores assinados, atualização automática e release privada.

O remoto Git `upstream` continuará apontando para o Simplicio Code para permitir
absorver correções de segurança e compatibilidade sem perder a camada Simplicio.
