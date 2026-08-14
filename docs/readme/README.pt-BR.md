<h1 align="center">LWC — Memória proativa para agentes de IA</h1>

<p align="center"><strong>Conduzido por agentes · Persistente · Baseado em fontes</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/@i-xor/lwc"><img alt="npm: @i-xor/lwc" src="https://img.shields.io/badge/npm-%40i--xor%2Flwc-CB3837?logo=npm"></a>
  <a href="https://crates.io/crates/lwc"><img alt="crates.io: lwc" src="https://img.shields.io/crates/v/lwc.svg"></a>
  <img alt="Node.js 22 ou mais recente" src="https://img.shields.io/badge/node-%3E%3D22-5FA04E?logo=nodedotjs">
  <img alt="Plataformas: macOS, Linux e Windows" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-666666">
  <a href="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://skills.sh/janyork/llm-wiki-cli/using-lwc"><img alt="skills.sh: using-lwc" src="https://img.shields.io/badge/skills.sh-using--lwc-000000?logo=vercel"></a>
  <a href="../../LICENSE"><img alt="Licença: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="../../README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a> · <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> · <a href="README.fr.md">Français</a> ·
  <a href="README.ru.md">Русский</a>
</p>

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-social-preview.png" alt="LWC — Memória proativa para agentes de IA" width="100%"></p>

`lwc` é uma CLI de memória proativa conduzida por agentes e feita para agentes de IA. Ela permite que os próprios agentes recuperem, mantenham e façam evoluir conhecimento persistente e rastreável até suas fontes entre diferentes sessões.

**Funciona com Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity e pi.**

O LWC transforma documentos selecionados em uma Wiki duradoura. O agente raciocina e sintetiza; o `lwc` preserva fontes, páginas, citações, links, índices e histórico para que o conhecimento se acumule, em vez de ser reconstruído a partir de trechos brutos a cada consulta.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-overview-en.png" alt="Visão geral do LWC" width="820"></p>

## LWC é memória para agentes, não RAG

RAG e LWC ajudam um LLM a trabalhar com documentos externos, mas mantêm o estado em lugares diferentes. Uma consulta RAG típica recupera trechos brutos e monta uma resposta pontual:

```text
query -> retrieve chunks -> generate answer
```

O LWC mantém o trabalho útil entre consultas:

```text
task -> recall maintained Wiki -> reason from sources and prior synthesis
     -> write durable improvements back
```

A recuperação é uma operação do LWC, não o princípio que organiza o produto. O artefato duradouro é uma Wiki baseada em fontes, cujas páginas, citações, links, contradições e histórico são revisados conforme o conhecimento muda. Por isso, o LWC não exige embeddings nem banco vetorial e não descarta cada síntese depois de responder. Ele pode complementar RAG, mas não é RAG executado no momento da consulta.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-source-grounding-en.png" alt="Fontes e rastreabilidade no LWC" width="820"></p>

### Quem opera o LWC é o agente

`lwc` é uma interface de máquina para agentes, não um aplicativo de anotações voltado a pessoas. No uso normal, uma pessoa seleciona fontes, define objetivos, faz perguntas e revisa respostas ou o Markdown projetado. O agente executa a CLI, gerencia escopos, integra fontes, mantém citações e links e decide o que vale a pena recuperar ou registrar de volta.

Não conduza manualmente o fluxo cotidiano do `lwc`, a menos que esteja desenvolvendo ou depurando a ferramenta. Peça ao agente para ativar o Skill canônico `using-lwc`, normalmente com `$using-lwc`.

## Recomendado: peça ao agente para configurar o LWC

Cole o prompt abaixo no agente que você usa. Ele instala a CLI global, delega a configuração dos hosts compatíveis ao instalador idempotente AgentTarget do LWC e só usa a configuração nativa quando o agente ainda não está registrado.

<details>
<summary><strong>Copiar o prompt completo de configuração</strong></summary>

```text
Configure o LWC por completo para este usuário. Execute e verifique o trabalho;
não se limite a descrever os comandos que eu deveria executar.

Fontes de referência:
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

Requisitos:
1. Leia este README, `SECURITY.md` e `skills/using-lwc/SKILL.md`. Se `lwc` não
   puder ser chamado globalmente, instale a versão oficial verificada por checksum;
   não prefixe comandos normais com um caminho privado de binário nem com
   `LWC_PROJECT_ROOT`.
2. Execute `lwc --version`; se a memória global não existir, inicialize-a uma vez
   com `lwc --scope global init`; depois execute `lwc agent install --yes`. Esse
   comando detecta agentes compatíveis instalados e configura MCP, Skill, Hook e
   Instructions com segurança nos locais oficiais. Não recrie essa lógica à mão
   nem instale também um pacote nativo para o mesmo agente.
3. Verifique `lwc agent status --target all --location global`. Reinicie os agentes
   afetados e conclua a revisão normal de confiança dos Hooks quando necessário.
   Não inicialize uma Wiki de projeto nem qualquer grafo sem consentimento explícito
   para o projeto.
4. Se o ambiente de execução atual não for um AgentTarget registrado pelo LWC, use as convenções
   oficiais de usuário desse ambiente para instalar o Skill canônico `using-lwc`, um
   bloco de instruções aditivo, `lwc serve --mcp` e um Hook de sessão limitado,
   somente onde houver suporte oficial. Preserve a configuração existente, mantenha
   a idempotência e relate pontos de integração sem suporte em vez de inventar caminhos ou chaves.

Ao final, informe a versão do LWC, os Targets detectados e configurados, os resultados
de status, os arquivos alterados, os pontos de integração sem suporte e qualquer reinicialização
ou ação de confiança ainda pendente.
```

</details>

## Origem e agradecimentos

`lwc` implementa o padrão [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f), proposto por Andrej Karpathy: um LLM constrói e mantém de forma incremental uma Wiki persistente e interligada, em vez de reconstruir conhecimento a partir de documentos brutos em toda consulta. A arquitetura da CLI e alguns detalhes também se inspiram em [`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki).

Este projeto adapta essas ideias para uma CLI Rust pensada primeiro para agentes e apoiada por SQLite.

## Projeto fundamental

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-architecture-en.png" alt="Arquitetura do LWC" width="100%"></p>

O modelo persistente de conhecimento tem três camadas lógicas:

| Camada | Conteúdo | Contrato |
| --- | --- | --- |
| Fontes brutas | Snapshots imutáveis de entradas selecionadas | Adicione por `source`; nunca reescreva a verdade da fonte. |
| Wiki | Páginas, citações, links e proveniência mantidos pelo agente | Atualize por `page`; cite fontes e classifique conhecimento duradouro que não vem de uma fonte. |
| Esquema e propósito | Regras de manutenção e intenção do projeto | Orientam cada ingestão e revisão futura. |

SQLite é a fonte canônica. A árvore Markdown é uma projeção reconstruível para pessoas e ferramentas como o Obsidian. Agentes alteram o conhecimento pelo `lwc`, não editando `.lwc/wiki.db` nem o Markdown projetado diretamente. Comandos bem-sucedidos retornam JSON em stdout; falhas retornam JSON estruturado em stderr.

Comandos de leitura mantêm os armazenamentos no formato atual em modo somente leitura. Quando uma CLI nova abre pela primeira vez um armazenamento antigo gravável, migra o schema uma vez, dentro de uma transação, antes de continuar a leitura.

## Recuperação hierárquica e grafo de conhecimento

Cada Source e página Wiki atual é indexada de forma determinística em passagens e frases. SQLite continua sendo a autoridade; span FTS e o grafo externo opcional são índices reconstruíveis. A busca existente continua retornando apenas documentos, a menos que uma granularidade seja pedida.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="Grafo de memória do LWC" width="100%"></p>

```bash
lwc search "projection consistency" --granularity sentence --type page
lwc search "projection consistency" --granularity passage
lwc search "projection consistency" --granularity all --group-by document
lwc span get <SPAN_ID>
lwc span expand <SPAN_ID> --before 1 --after 1 --children 20
```

Localizadores de spans incluem a impressão digital do documento e a versão da segmentação. Um localizador cujo corpo foi substituído falha com `stale_span` e informa metadados anteriores e atuais; o LWC nunca o remapeia silenciosamente para texto semelhante.

Use a API de grafo tipada e limitada para explorar sem palavras-chave:

```bash
lwc graph explore                         # representative macro view
lwc graph node page:projection-policy
lwc graph neighbors page:projection-policy --direction outgoing
lwc graph path page:implementation page:policy --max-depth 6
lwc graph impact page:policy --max-depth 4
lwc graph overview
lwc graph status
lwc graph verify
```

Arestas automáticas se limitam a fatos estruturais ou sustentados por evidência. Relações semânticas precisam ser explícitas e auditáveis:

```bash
lwc graph relation set page:implementation DEPENDS_ON page:policy \
  --provenance source-grounded --source 12 \
  --reason "Source 12 states the required policy" --confidence 0.95
lwc graph relation list --from page:implementation
lwc graph relation retract page:implementation DEPENDS_ON page:policy \
  --reason "The dependency was superseded"
```

Os motivos das relações são conteúdo duradouro: nunca inclua credenciais, segredos ou cadeia de pensamento bruta.

Os documentos SQLite continuam sendo a autoridade. O armazenamento do grafo vem desativado; habilite exatamente um engine externo quando precisar percorrê-lo. A configuração combina, em camadas, valores internos, globais e do projeto:

```bash
lwc config show
lwc config set --graph grafeo
lwc config set --graph surrealdb
lwc config set --graph disabled
lwc config unset --graph
```

A conversão para Markdown é uma operação opcional separada. `lwc init` mostra a mesma orientação legível por máquina, mas nunca instala nem habilita um conversor. Instale um adaptador, selecione-o explicitamente, converta para um novo arquivo Markdown local, revise e só então faça a ingestão:

```bash
# Choose one adapter; both are disabled unless configured.
npm install --global @firecrawl/anydoc
lwc config set --trans anydoc

# Or:
python3 -m pip install 'markitdown[all]'
lwc config set --trans markitdown

lwc trans INPUT --output OUTPUT.md
lwc source add OUTPUT.md
```

A configuração aceita `--trans-timeout 1..900` e várias opções `--trans-arg=<value>`. O LWC executa diretamente o binário fixo escolhido, nunca cai para o outro adaptador, só aceita arquivos locais, limita entrada e saída a 64 MiB e nunca sobrescreve uma saída existente. Guarde credenciais no ambiente do adaptador, não na configuração do LWC. Consulte a documentação oficial do [Anydoc](https://github.com/firecrawl/anydoc) e do [MarkItDown](https://github.com/microsoft/markitdown) para formatos e opções.

Grafeo e o SurrealDB embarcado usam armazenamentos auxiliares descartáveis em `.lwc/`. Cada Work `graph-project` confirma uma Source/Page atual e seus links, citações e relações explícitas antes de iniciar o próximo documento. Atualizações e exclusões enfileiram somente documentos afetados; rebuild e resume usam as mesmas unidades. Revisões históricas permanecem imutáveis e nunca são tokenizadas ou projetadas de novo. Acompanhe por `work list`, `work status` ou `work watch`, e use `work resume` após interrupções. `graph status` informa o engine e a quantidade de documentos projetados; `graph verify` compara as chaves atuais com o SQLite.

## Instalação

A maioria das pessoas deve usar o prompt de configuração acima. Os comandos manuais servem para manutenção, depuração ou ambientes que não podem instalar o Skill complementar.

Homebrew (há Bottles para macOS Apple silicon e Linux x86_64):

```bash
brew install JanYork/tap/lwc
```

npm (Node.js 22+):

```bash
npm install --global @i-xor/lwc
```

crates.io:

```bash
cargo install --locked lwc
```

GitHub:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | sh
```

O instalador aceita macOS x86_64/aarch64, Linux glibc e Windows Git Bash, verifica o checksum e instala ou atualiza `lwc`. O padrão é `~/.local/bin`; uma cópia existente em `~/.local/bin` ou `~/.cargo/bin` é atualizada. Para escolher outro diretório:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | LWC_INSTALL_DIR="$HOME/bin" sh
```

Ou compile do GitHub com Cargo:

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

Ou instale a partir de uma cópia local do repositório:

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## Skill complementar para agentes

O repositório inclui [`skills/using-lwc`](../../skills/using-lwc), um Agent Skill que torna `lwc` uma camada de memória proativa em sessões substanciais. Instale pelo [skills.sh](https://skills.sh/JanYork/llm-wiki-cli):

```bash
npx skills add JanYork/llm-wiki-cli --skill using-lwc -g
```

Também é possível copiar de uma cópia local do repositório para o diretório de Skills do ambiente de execução. Para Codex:

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/using-lwc "$HOME/.agents/skills/"
```

A invocação canônica é `$using-lwc`.

Quando ativado, o Skill:

- encontra uma CLI compatível ou instala a versão oficial verificada por checksum;
- inicializa uma vez a memória global em `~/.lwc/`;
- recupera contexto global e de projeto limitado antes de repetir uma investigação;
- inicializa o projeto ativo quando chamado explicitamente e, caso contrário, pergunta primeiro;
- recusa gravações de projeto fora da raiz autorizada do workspace atual;
- separa fatos do projeto de conhecimento global reutilizável;
- integra fontes e registra respostas duradouras de volta na Wiki.

`SKILL.md` é um roteador curto, não um manual monolítico. Ele aponta para documentos específicos sobre memória básica, momento de ativação, memória ativa, grafo físico, Word Graph limitado, CodeGraph, strong tags, conversão, onboarding e recuperação/manutenção. Cada documento diz quando usar ou pular o recurso, o fluxo mínimo, o limite de consentimento e a evidência de conclusão.

O Skill normalmente descobre o projeto pelo diretório atual e chama diretamente o `lwc` global. `LWC_PROJECT_ROOT` é um limite explícito para um projeto escolhido deliberadamente, não um prefixo para comandos cotidianos no projeto atual.

Defina `LWC_AUTO_INSTALL=0` para desativar a instalação automática. Ela executa o instalador revisado incluído no Skill, confia neste repositório e no perímetro de publicação do GitHub Releases e compara o arquivo com `SHA256SUMS`; checksum protege integridade, não é assinatura do editor. Os binários cobrem macOS x86_64/aarch64, Linux glibc e Windows via Git Bash. `SKILL.md` segue o layout de Agent Skills e `agents/openai.yaml` fornece metadados para OpenAI/Codex. A CLI não depende do ambiente de execução: qualquer agente que possa executá-la e carregar ou adaptar as instruções pode usar o LWC. Comandos do Skill, instruções globais e Hooks são específicos de cada ambiente; por isso o prompt detecta e configura o host atual.

### Configuração nativa de agentes

O LWC detecta agentes compatíveis e instala um único MCP LWC somente leitura. Os 12 AgentTargets registrados são adaptadores completos: instalam todos os pontos de integração oficiais baseados em arquivos — MCP, Skill, Hook e Instructions — disponíveis em cada host e escopo, e informam claramente os que são controlados pela interface, estão em prévia ou não têm suporte.

```bash
lwc agent install --yes
lwc agent status --target all --location global
lwc agent install --print-config codex
lwc agent refresh --target codex,claude
lwc agent uninstall --target codex,claude --yes
```

`--yes` seleciona agentes detectados, escopo global e Hooks padrão de ciclo de vida/prompt. Use `--no-prompt-hook` para omitir o Hook por prompt do Claude. A entrada instalada é `lwc -> serve --mcp`; a única ferramenta, `lwc_explore`, lê por padrão memória Wiki limitada e aceita modos explícitos `code` e `all`. O `projectPath` precisa ficar dentro do workspace onde o host MCP iniciou o LWC. A ferramenta nunca baixa nem inicializa CodeGraph. Install e refresh repetidos são idempotentes byte a byte; uninstall só restaura estado pertencente ao LWC e mantém índices do projeto. Pacotes opcionais para Codex, Claude Code e Pi ficam em `integrations/`. Instalar um pacote não concede nem contorna confiança nativa. Não combine instalação direta e pacote nativo para o mesmo agente. Cada pacote traz o Skill `using-lwc` completo, sem depender de gerenciador externo ou do ambiente do mantenedor.

Como Pi não tem MCP embutido, ele expõe o MCP LWC pela ponte oficial de extensão. Os demais Targets só registram `lwc serve --mcp`; CodeGraph permanece um plano interno de contexto e não vira um segundo MCP. Confiança e permissões controladas pela interface continuam sob responsabilidade do usuário. Os pontos de integração em prévia são identificados; escopos parciais instalam o que é suportado sem degradar ou rejeitar o Target inteiro. Caminhos globais do Kiro respeitam `KIRO_HOME`.

A interface de Target, ordem de registro, detecção e caminhos MCP seguem o design do adaptador do CodeGraph, licenciado sob MIT. O LWC acrescenta MCP unificado, status por superfície, Skills e Hooks, propriedade de arquivos compartilhados e rollback exato. Consulte [`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md).

A saída de `lwc init` e os Hooks de início/compactação expõem fatos `LWC_READINESS` limitados sobre Wiki, grafo físico, runtime e índice do CodeGraph e integração de agentes. O estado do grafo físico separa consentimento configurado de projeção pendente ou com falha. A detecção é somente leitura e nunca habilita nem inicializa grafos. Quando ambos precisam de autorização, a base portátil é texto simples:

```text
1. Enable physical document graph and CodeGraph (recommended)
2. Enable physical document graph only
3. Enable CodeGraph only
4. Later
```

Após escolher `1`, o agente inicializa uma Wiki ausente, habilita Grafeo, aguarda e verifica o Work de projeção, inicializa CodeGraph e confere os dois resultados separadamente. `Later` não altera nada nem bloqueia a tarefa. Plugins podem mostrar os mesmos IDs em sua interface; checkboxes não são obrigatórios.

Strong tags carregam integralmente poucas regras ou runbooks, de forma explícita e limitada:

```bash
lwc tag set "operations" incident-response --priority 100 --reason "primary runbook"
lwc load tag "operations" --limit 3
lwc tag autoload "operations" --enable --priority 100 --limit 3 \
  --max-chars 50000 --reason "required at session boundaries"
```

Não é busca derivada de tokens: limites e orçamento de caracteres são aplicados antes de páginas completas entrarem no contexto.

## Início rápido

Esta seção descreve o protocolo executado pelo agente. Pessoas não precisam rodar estes comandos no uso normal.

### 1. Inicializar uma Wiki de projeto

```bash
cd your-project
lwc init
printf '# Schema\nEvery page declares provenance; source-grounded claims cite sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

A inicialização adiciona `.lwc/` ao `info/exclude` local do Git quando necessário, sem mudar `.gitignore`. Use `lwc init --no-git-exclude` somente quando a Wiki for versionada intencionalmente.

### 2. Adicionar material de origem

```bash
lwc source add-dir docs/
```

Arquivos sem título explícito usam a origem como fallback estável e legível. Bytes idênticos são deduplicados por SHA-256. Fontes resolvidas fora da raiz ativa exigem `--allow-external-source`. Marcadores de credenciais de alta confiança são recusados, a menos que a fonte revisada seja reconhecida com `--acknowledge-sensitive-source`.

Cada adição registra também o caminho observado e o snapshot imutável atual. Antes de confiar em evidência baseada em arquivo, verifique apenas as fontes relevantes:

```bash
lwc source status 7 12
```

O comando transmite cada arquivo atual por SHA-256 e informa separadamente a linhagem (`current` ou `superseded`) e o estado do sistema (`current`, `modified`, `missing`, `unreadable`, `oversized` ou `unstable`). É somente leitura. Use `source status --all` apenas para manutenção explícita, pois o custo é proporcional a todos os bytes rastreados. Revise um caminho modificado antes de atualizar conhecimento:

```bash
lwc source diff 7
lwc source refs 7 --limit 1000
```

`source diff` compara a fonte imutável com o arquivo atual ou outro snapshot via `--to-source`. O diff é limitado a 8 MiB e 200.000 linhas por lado, 20.000 caracteres Unicode por padrão e 100.000 com `--max-chars`. Se uma fonte foi observada em vários caminhos, escolha um `--path`. Um diff truncado é só prévia. `source refs` lista candidatos que citam diretamente a fonte; não prova impacto semântico. Rode `source add` novamente só após revisar uma nova revisão relevante. A sequência A -> B -> A mantém três observações, mesmo reutilizando o source ID de A. Caminhos externos exigem novamente `--allow-external-source`; texto sinalizado também exige `--acknowledge-sensitive-source` após revisão.

Fontes migradas de armazenamentos antigos ficam explicitamente sem rastreamento porque o LWC não adivinha caminhos históricos. Adicione o arquivo uma vez para criar a primeira revisão. Se o arquivo ou a revisão mais recente do caminho mudar durante a checagem, o LWC retorna `source_status_unstable`; tente novamente.

Para importação atômica, caminhos de um manifest JSON são resolvidos a partir do diretório do manifest:

```json
{
  "sources": [
    {"path": "ARCHITECTURE.md", "title": "Architecture contract"},
    {"path": "src/store.rs", "title": "SQLite store"}
  ]
}
```

```bash
lwc source add-manifest lwc-sources.json
```

### 3. Analisar e integrar uma fonte

```bash
lwc ingest next --context-limit 50 --source-max-chars 100000
lwc ingest analyze 1 --file analysis.md
```

Use `lwc ingest claim 7` quando um manifest ou scheduler já selecionou um source ID pendente.

Se `source_window.has_more` for true, continue em `source_window.next_offset_chars`:

```bash
lwc source show 1 --offset-chars 100000 --max-chars 100000
```

Antes de concluir, crie uma página source-summary citada e integre a contribuição em pelo menos uma página não source:

```bash
lwc page put source-1 \
  --title "Source 1 Summary" \
  --kind source \
  --summary "What this source contributes" \
  --file source-summary.md \
  --source 1

lwc page put durable-concept \
  --title "Durable Concept" \
  --kind concept \
  --summary "How this source changes shared knowledge" \
  --file concept.md \
  --source 1

lwc ingest complete 1
```

As duas camadas são necessárias: a página source ajuda em navegação e proveniência; a página compartilhada faz o conhecimento acumular. Se uma fonte realmente não muda página compartilhada, conclua com explicação específica e auditável:

```bash
lwc ingest complete 1 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

Citações geram automaticamente proveniência `source-grounded`. Para conhecimento vindo do usuário, observação do agente ou hipótese, repita `--provenance` em vez de inventar uma fonte:

```bash
lwc page put architecture-decision \
  --title "Architecture decision" \
  --kind query \
  --summary "Accepted constraint and remaining uncertainty" \
  --file decision.md \
  --provenance user-provided \
  --provenance hypothesis
```

`page put` substitui todo o conjunto de citações e proveniência explícita. Leia a página existente e repita cada `--source` e `--provenance` ainda válido. Não passe `source-grounded`: ele é derivado das citações. Proveniência aparece em page, context, search, refs e projeção, mas não altera ranking.

### 4. Consultar a Wiki acumulada

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc search "question keywords" --limit 20 --explain
lwc search "concept only" --type page --kind concept
lwc search "exact evidence" --type source
lwc page show source-1
```

## Fluxo de trabalho do agente

1. Colete fontes imutáveis.
2. Reivindique uma tarefa com `lwc ingest next`, ou `ingest claim <ID>` quando a fonte já estiver escolhida.
3. Leia todas as janelas, schema, purpose e contexto limitado.
4. Analise antes de gerar páginas.
5. Crie ou revise resumo e páginas compartilhadas com citações `--source` explícitas.
6. Conclua apenas após as duas condições, ou registre por que nenhuma página deve mudar.
7. Coloque ingestão multicomando ou revisão ampla em um changeset, valide o rascunho e publique atomicamente.
8. Use `search`, `context`, `graph` e `lint` para manter coerência.

Veja [docs/agent-workflow.md](../../docs/agent-workflow.md) para o contrato completo. `lwc --help` e `lwc <command> --help` mostram pré-condições, transições, efeitos e próximas ações.

## Alterações atômicas com vários comandos

Um comando `source` ou `page` é transacional. Use changeset quando uma atualização precisar de vários comandos sem expor uma Wiki parcial:

```bash
lwc --scope project changeset begin architecture-refresh
lwc --scope project --changeset architecture-refresh source add-manifest sources.json
lwc --scope project --changeset architecture-refresh ingest claim 1
# Analyze, write cited pages, and complete ingest with the same selector.
lwc --scope project --changeset architecture-refresh lint
lwc --scope project --changeset architecture-refresh search "expected answer" --limit 5
lwc --scope project changeset show architecture-refresh
lwc --scope project changeset commit architecture-refresh
```

Leituras do rascunho veem gravações preparadas; SQLite e Markdown ativos não mudam. O banco começa como overlay esparso pequeno, sem copiar ou criar checkpoint da Wiki. `changeset show` relata operações, revisões e prontidão sem lint. Commit valida e aplica apenas entidades afetadas, preservando gravações não relacionadas; conflito na mesma entidade falha sem sobrescrever lados. Rascunhos vazios e problemas de lint são recusados; não há force nem merge automático. Use `--allow-lint-issues --reason "reviewed pre-existing debt"` só para dívida auditada que não foi introduzida. Depois do commit, repita as mesmas buscas no estado ativo. Commit congela o rascunho; `changeset_frozen` bloqueia novas gravações. Tente o mesmo commit para recuperação ou descarte após conflito — não acrescente trabalho.

```bash
lwc --scope project changeset discard architecture-refresh
lwc --scope project changeset rollback <CHANGESET_ID>
```

Discard só afeta rascunho sem commit. Commit grava patch inverso com checksum e retorna o ID exato; rollback restaura só essas entidades e recusa se alguma mudou. Changesets project/global são separados, `--scope all` é inválido, e `init`, `maintenance`, `checkpoint` e changesets aninhados recusam `--changeset`. Rascunhos não criam segunda projeção. Se erro indicar `committed=true` com cleanup ou materialização pendente, não repita a mudança; execute a recuperação indicada.

Commit esparso tem patches exatos para Source add/ingest, Page put/remove, schema, purpose e buscas registradas. Pesos e relações semânticas falham antes de checkpoint, lock ou mudança ativa com `changeset_sparse_unsupported`; aplique-os como transações diretas até haver patches inversos.

## Escopos

| Escopo | Store | Uso |
| --- | --- | --- |
| `project` | `.lwc/wiki.db` no ancestral mais próximo | Padrão; conhecimento do projeto |
| `global` | `~/.lwc/wiki.db` | Conhecimento reutilizável |
| `all` | project e global | Apenas `search` e `context` combinados |

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
```

Gravações são explícitas. `all` não cria citações ou links entre armazenamentos; `search --record` só adiciona a operação a cada armazenamento selecionado.

## Busca e CJK

A busca é lexical e determinística.

- Termos são texto simples, não FTS bruto.
- `--type auto` prioriza páginas compiladas, oculta fontes pareadas e usa fontes como fallback.
- Use `--type page`, `--type source` ou `--type all`; repita `--kind` para limitar tipos.
- Termos CJK usam bigramas adjacentes; unigramas não vazios mantêm buscas de um caractere.
- Texto latino vira tokens alfanuméricos minúsculos.
- Título, nome de arquivo, path/slug, resumo e corpo são avaliados separadamente; título e caminho recebem aumentos limitados de pontuação.
- README, índices, visões gerais e documentos centrais de navegação são rebaixados conforme a consulta; pedir o README desativa a penalidade.
- Candidatos podem receber um aumento de pontuação por link direto ou fonte compartilhada. Vizinhos comuns sozinhos não mudam a ordem, e documentos de navegação genéricos recebem penalidade.
- `--explain` retorna a aritmética exata de sinais lexicais, genéricos, grafo, peso manual e feedback. Só `--record` registra a consulta.
- Coeficientes fixos e a regra «quanto menor a pontuação, maior a relevância» mantêm project/global comparáveis em `--scope all`.

Não há dependência de dicionário de segmentação, para manter estabilidade com nomes de produto, codinomes, termos mistos e vocabulário novo.

### Pesos e feedback explícitos

Use peso documental para julgamento duradouro e independente da consulta. Use feedback para a impressão exata de tokens ordenados:

```bash
lwc weight set page payment-rules \
  --value 2 \
  --reason "Canonical payment rules specification" \
  --provenance agent-observed
lwc weight list page payment-rules

lwc weight feedback page payment-rules \
  --query "payment reconciliation rules" \
  --signal relevant \
  --reason "Verified against the expected answer" \
  --provenance agent-observed

lwc weight feedback-clear page payment-rules \
  --query "payment reconciliation rules" \
  --provenance agent-observed
lwc weight clear page payment-rules --provenance agent-observed
```

Valores são `-2`, `-1`, `1` e `2`; use `clear` para zero. Ambos só reordenam candidatos lexicais. `user-provided` prevalece sobre `agent-observed`, mantendo ambos auditáveis. Feedback guarda SHA-256, não a consulta, e não transfere para paráfrases com tokens diferentes. Motivos e operações são duradouros: não copie consulta sensível para `--reason`. Mutações exigem `project` ou `global`; `--scope all` é recusado.

## Visualizador somente leitura e CodeGraph

`lwc view` inicia em primeiro plano um inspetor limitado ao loopback e abre o navegador. Ele serve um único app TS + Lit embarcado, sem CDN ou runtime Node durante o uso, e só expõe APIs GET/HEAD. Páginas, fontes, Markdown, grafo de conhecimento e grafo de código opcional são lidos do projeto atual sem migração, refresh ou construção:

```bash
lwc view
lwc view --port 4173 --no-open
```

O visualizador começa em inglês. Use `中文` / `EN` para alternar; o navegador lembra a escolha e o conteúdo da Wiki permanece no idioma original. Os grafos usam uma visão 3D inspirada no Obsidian, com nós pequenos, rótulos persistentes, links finos, rotação e zoom.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="Inteligência de código do LWC CodeGraph" width="100%"></p>

Indexação de código existe somente em project e fica desativada até inicialização explícita. O fork fixado do CodeGraph é baixado uma vez do GitHub Releases, verificado com SHA-256 e guardado em `~/.lwc/runtime/codegraph/<PIN>/<TARGET>/`; cada projeto mantém só o índice em `.lwc/codegraph`. Telemetria fica sempre desligada e nenhum estado `.codegraph` é usado.

```bash
lwc cg status
lwc cg init                 # download once, then index one complete file at a time
lwc cg sync
lwc cg query UserService
lwc cg node UserService
lwc cg callers UserService
lwc cg callees UserService
lwc cg impact UserService
lwc cg files
```

A versão fixada do runtime reconhece estas linguagens e formatos relacionados a código: TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust, Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro, Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig, XML, `.properties`, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang e Terraform. YAML, Twig e `.properties` são rastreados no nível do arquivo; resolvedores de frameworks ainda podem adicionar relações. XML é reconhecido para extrair mappers MyBatis.

`lwc cg` encaminha todas as consultas do CodeGraph. Comandos globais de ciclo de vida (`install`, `uninstall`, `upgrade`, `telemetry`, `daemon`, `daemons`) são bloqueados. A ponte `lwc cg serve --mcp` permanece para compatibilidade manual antiga; integrações novas usam `lwc serve --mcp`, reunindo exploração limitada de Wiki e CodeGraph em uma ferramenta somente leitura. O LWC controla o runtime e impõe o limite do projeto. Gravações iniciais, incrementais, completas, de atualização, exclusão, resolução e recuperação confirmam integralmente cada arquivo ao qual pertencem antes do próximo; o grafo atual continua legível e revisões históricas não são atualizadas.

## Manutenção e projeção

```bash
lwc lint
lwc maintenance reindex
lwc maintenance materialize
lwc maintenance compact
lwc work list
lwc work status <WORK_ID>
lwc work watch <WORK_ID>
lwc work cancel <WORK_ID>
lwc work resume <WORK_ID>
lwc checkpoint create before-large-update
lwc checkpoint list
lwc log --limit 20
```

- Comandos de manutenção retornam um `work` duradouro imediatamente. Leia com `work status` ou espere com `work watch` e confira `work.result`. A migração schema v10-v11 usa o mesmo mecanismo; comandos normais não a executam inline.
- `lint` é somente leitura por padrão. Adicione `--record` só quando a checagem deve entrar no histórico.
- `maintenance reindex` reconstrói artefatos de busca derivados do SQLite.
- `maintenance materialize` reconstrói a árvore Markdown projetada.
- `maintenance compact` só tenta checkpoint WAL truncate; não esconde otimização FTS. Execute com a Wiki ociosa e confira `busy` e `after_bytes`. Reader ocupado retorna sem mudar conteúdo canônico.
- Consultas são privadas por padrão; use `--record` só quando quiser gravar o texto.

`lwc checkpoint create <NAME>` usa a API de backup em linha do SQLite. Restaure com `lwc checkpoint restore <NAME>`; o LWC primeiro cria `pre-restore-*` e depois reconstrói a projeção. Use `source remove <ID>` e `page remove <SLUG>` para exclusão protegida: fontes citadas e páginas com links de entrada são recusadas. Excluir a fonte atual de um caminho rastreado encerra o rastreamento sem expor uma revisão antiga como atual.

Para ingestão de várias fontes ou substituição ampla, prefira changeset a checkpoint manual: commit grava patch inverso esparso, publica só entidades afetadas em uma transação e materializa Markdown incrementalmente. Depois tenta truncar o WAL; `wal_checkpointed=false` indica um processo de leitura ativo, não uma falha no commit canônico.

Para backup externo, pare comandos `lwc` ativos e copie `.lwc/` inteiro. Não copie apenas `wiki.db` enquanto um processo de escrita puder usar o WAL.

## Suíte de benchmarks

O benchmark opcional importa um corpus UTF-8 local para uma Wiki temporária e mede importação, busca P50/P95, Recall@5/10, MRR e armazenamento antes/depois de compactar. O conjunto de referência é um JSONL de consultas e caminhos esperados:

```bash
cargo build --release
LWC_BENCH_CORPUS=/path/to/sanitized-corpus \
LWC_BENCH_QUERY_SET=/path/to/query-set.jsonl \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
cargo test --test search_benchmark -- --ignored --nocapture
```

`cargo test --all-targets` cobre busca page-first, filtros type/kind, janelas UTF-8, condições de ingestão, precisão do grafo, migrações, lint e compactação WAL. Veja [benchmarks/README.md](../../benchmarks/README.md) para contrato e comparação justa.

## Limites e não objetivos

Restrições atuais:

- base de conhecimento para uma máquina e um usuário;
- fluxo de texto UTF-8;
- limite de 64 MiB por schema, purpose, source ou corpo de página;
- busca lexical, não recuperação vetorial semântica.

Fora de escopo de propósito:

- sem chamadas LLM embutidas;
- sem banco vetorial;
- sem daemon ou serviço em segundo plano;
- sem interface web ou desktop;
- sem contrato de edição direta do banco.

Se a projeção Markdown divergir, reconstrua. Se o schema SQLite estiver errado, corrija pela CLI e migrações, não à mão.

## Como contribuir

Issues e pull requests são bem-vindos, especialmente sobre:

- ergonomia do fluxo de agentes;
- projeção determinística;
- contratos duradouros de citações e manutenção;
- qualidade de busca em corpus técnicos multilíngues.

Leia [CONTRIBUTING.md](../../CONTRIBUTING.md) antes de abrir um pull request. Relate segurança conforme [SECURITY.md](../../SECURITY.md).

## Licença

Licenciado sob a [Apache License 2.0](../../LICENSE).
