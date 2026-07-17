# Contrato de API: site_simpleti como backend de conta e assinatura

Status: **rascunho de contrato, não implementado**. Este documento e o
`site-simpleti-openapi.yaml` ao lado definem a fronteira entre o
`simplicio-code` (este repositório, cliente) e o backend de conta/assinatura
que vive em `site_simpleti` (outro repositório, ao qual este agente não tem
acesso). Nada aqui assume que o backend já existe; o objetivo é dar às duas
equipes algo concreto para convergir, conforme a issue #15.

## Por que este contrato vive aqui

`simplicio-code` deve permanecer desacoplado do backend por contrato: o
cliente não conhece detalhes de implementação do `site_simpleti` (banco de
dados, provedor de pagamento, nomes internos de modelo/upstream), apenas a
forma da API pública. Definir o contrato a partir do lado do cliente também
documenta explicitamente o que o cliente **nunca** deve receber ou enviar
(ver "Invariantes" abaixo), o que é mais fácil de auditar em um único
repositório pequeno do que espalhado pelo backend.

Ver também `docs/ARCHITECTURE.md`, seção "Gateway Simplicio", que já descrevia
o fluxo de alto nível antes deste contrato ser formalizado.

## Escopo coberto pelo contrato

1. `POST /auth/device/authorize` — inicia o device flow.
2. `POST /auth/device/token` — troca `device_code` aprovado por tokens.
3. `POST /auth/token/refresh` — renova o access token.
4. `POST /auth/token/revoke` — logout / revogação explícita.
5. `GET /entitlement` — plano, status e profiles liberados.
6. `GET /devices`, `DELETE /devices/{deviceId}` — gestão de dispositivos.
7. `POST /webhooks/subscription` — eventos de ciclo de vida da assinatura.

O schema formal de cada endpoint está em
[`site-simpleti-openapi.yaml`](./site-simpleti-openapi.yaml) (OpenAPI 3.1).

## Invariantes de aceitação

- **Device codes de uso único e curta duração.** `device_code` e `user_code`
  expiram em no máximo 15 minutos (`expires_in`) e não podem ser trocados por
  token mais de uma vez: uma segunda tentativa de troca do mesmo
  `device_code` deve retornar `expired_token` ou `access_denied`, nunca um
  novo token válido.
- **Nenhum vazamento de provedor/modelo interno.** `EntitlementResponse.profiles`
  e `DeviceAuthorizeResponse` só contêm nomes de produto Simplicio (ex.:
  `"Simplicio-1"`). Nenhum campo do contrato carrega nomes como `grok`,
  `openrouter`, `opencode-go`, `opencode-zen` ou qualquer identificador de
  upstream. Isso espelha a decisão já registrada em `docs/ARCHITECTURE.md`.
- **Webhooks idempotentes.** A entrega é *at-least-once*; o mesmo `event_id`
  pode chegar mais de uma vez. O receptor deve processar o efeito colateral
  (atualizar entitlement local, etc.) no máximo uma vez por `event_id`, e
  responder 200 tanto na primeira entrega quanto em reentregas reconhecidas
  como duplicadas — nunca reprocessar nem retornar erro para uma duplicata
  legítima.
- **Revogação idempotente.** `POST /auth/token/revoke` e
  `DELETE /devices/{deviceId}` retornam sucesso (204) mesmo se o alvo já
  estiver revogado; revogar duas vezes não é erro.

## O que este repositório implementa agora

Como o backend em `site_simpleti` ainda não existe (ou não é acessível a
partir deste agente), esta fatia entrega apenas a parte que é
genuinamente implementável e testável do lado do cliente, sem servidor real:

- `crates/codegen/simplicio-account-client`: tipos Rust (`serde`) que
  espelham exatamente os schemas do OpenAPI acima, para que qualquer client
  HTTP futuro tenha os tipos prontos e o schema não fique só em YAML solto.
- Dentro do mesmo crate, o módulo `idempotency`: uma implementação pura
  (sem I/O, sem HTTP) da regra "processar um evento de webhook no máximo uma
  vez dado um `event_id`", com testes reais cobrindo entrega duplicada,
  entregas de eventos diferentes e comportamento do "seen store" em memória.
  Este é o único pedaço de lógica de negócio da issue que não depende do
  backend existir para ser correto e testável.

## O que fica em aberto (a maior parte da issue)

Isto é deliberadamente a maior parte do trabalho da issue #15, porque ela é
uma feature cross-repositório:

- Implementação real do backend em `site_simpleti` (todos os endpoints acima).
- Um client HTTP real neste repositório que fale com esse backend (hoje não
  existe nenhuma camada HTTP para consumo de API externa autenticada; existe
  apenas o cliente MCP para o Runtime local, em
  `crates/codegen/simplicio-runtime-client`, que é um protocolo diferente).
- `simplicio-code login` de fato abrindo o browser e executando o device flow
  contra um servidor real.
- Persistência de refresh token no keychain do sistema.
- Enforcement de entitlement/quota no caminho de `POST /v1/code/responses`
  (gateway de inferência), que ainda nem existe neste repositório.
- Testes de contrato end-to-end contra uma instância real (ou um mock server
  dedicado) do `site_simpleti` — o que exigiria acesso a esse repositório.
- Abertura de issue espelho em `site_simpleti` com link cruzado para a issue
  #15 aqui (passo 9 da issue original). Este agente não tem acesso a esse
  repositório e não pode abrir essa issue; ver o corpo do PR para o
  lembrete explícito de que isso precisa ser feito manualmente.

## Versionamento

Mudanças que quebram compatibilidade (remover campo obrigatório, mudar tipo,
remover endpoint) exigem incrementar `info.version` no YAML e coordenar o
release entre os dois repositórios antes de o cliente adotar a nova versão.
Mudanças aditivas (novo campo opcional, novo endpoint) não exigem bump maior.
