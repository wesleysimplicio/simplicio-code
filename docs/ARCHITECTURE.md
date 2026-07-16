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

O Runtime é a autoridade de leitura, inclusive quando um cliente ACP anuncia
filesystem próprio. `SimplicioRuntimeFs` não possui fallback
local: falha de instalação, identidade, protocolo, sandbox ou truncamento
interrompe a leitura. O cliente mantém uma sessão MCP por workspace e reinicia
a conexão após falha recuperável.

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

- Concluído: leitura de arquivos do agente via Runtime.
- Próximo: busca, edição e execução passam pelos respectivos tools MCP.
- Depois: identidade/entitlement Simplicio e gateway único de inferência.
- Por último: instaladores assinados, atualização automática e release privada.

O remoto Git `upstream` continuará apontando para o Grok Build para permitir
absorver correções de segurança e compatibilidade sem perder a camada Simplicio.
