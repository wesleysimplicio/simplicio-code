# AGENTS.md — simplicio-code

## Simplicio Ecosystem Contract (canonical)

This repository is a Simplicio ecosystem component. For every non-trivial task: run `simplicio runtime map --repo . --for-llm markdown`, then `simplicio memory "<task>"`, rank/load relevant skills, execute through the native `simplicio` CLI, validate, and record evidence. MCP is fallback transport only.

### Component boundaries
Use `simplicio-mapper` for bounded context, `simplicio-fast` for snapshots and PlanDAG, `simplicio-dev-cli` for deterministic implementation, `simplicio-runtime` for contracts/gates/validation/receipts, `simplicio-loop` for convergence and close-gates, and `simplicio-agent` as control plane. Providers are workers, never authorities.

### Execution and evidence
Use `simplicio`/`simplicio shell compact` for inspection and `simplicio edit --plan` or governed dev-cli for mutation. Preserve `simplicio.io/v1`; run `simplicio contracts smoke --json`, focused tests and `simplicio validate "<task>" --repo . --json`; close only with `simplicio evidence`. Facts are `MEASURED|` only with receipts, otherwise `UNVERIFIED|`; savings come only from `simplicio savings report --repo . --json`. Missing dependencies fail closed; never fabricate context or output.

<!-- simplicio-global-llm-architecture-rules:start -->
## Regras arquiteturais obrigatórias para qualquer LLM

Estas regras valem para análise, planejamento, implementação, revisão, testes,
release e documentação neste ecossistema. O agente deve lê-las antes de agir:

1. **Não mantenha compatibilidade retroativa.** O que está obsoleto deve ser
   deletado diretamente. Não adicione camadas de compatibilidade, migrações ou
   fallbacks para preservar comportamento antigo.
2. **Escolha a implementação mais simples que atende à necessidade atual.**
   Não crie abstrações preventivas nem camadas de configuração desnecessárias.
3. **Divida o sistema em camadas longas.** Faça primeiro uma versão mínima
   end-to-end funcionando; depois adicione capacidades por cima. Não desmonte
   algo que funciona por complexidades inacabadas.
4. **Mantenha os componentes modulares**, com responsabilidades claramente
   separadas e limites explícitos.
5. **Priorize bibliotecas maduras e mantidas.** Não reescreva do zero sem
   motivo técnico explícito e registrado.
6. **Inspecione primeiro as dependências existentes.** Antes de adicionar um
   pacote ou escrever uma solução própria, verifique o que o projeto já possui.
7. **Decida a arquitetura pensando no longo prazo.** Não aceite soluções
   temporárias com a intenção de mudar depois.
8. **Use padrões de produtos maduros.** Pesquise como soluções consolidadas
   resolvem o mesmo problema e reutilize padrões validados; não reinvente a roda.

<!-- simplicio-global-llm-architecture-rules:end -->

