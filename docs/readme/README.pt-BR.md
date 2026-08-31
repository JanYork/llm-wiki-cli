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

**Funciona com Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity, GitHub Copilot in VS Code, Copilot CLI, Copilot for JetBrains e pi.**

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

O LWC separa o conhecimento durável em camadas com responsabilidades claras:

| Camada | Finalidade |
| --- | --- |
| Fontes originais | Snapshots imutáveis de evidências selecionadas |
| Wiki | Páginas, citações, links e proveniência mantidos pelo agente |
| Esquema e propósito | Regras do projeto que orientam a manutenção futura |

O SQLite é a fonte canônica. Markdown, índices de texto completo e grafos
opcionais são projeções reconstruíveis. As operações retornam JSON estruturado
para facilitar auditoria e recuperação.

[Conheça a arquitetura →](https://github.com/JanYork/llm-wiki-cli/wiki/Architecture-Overview)

## Recuperação hierárquica e grafo de conhecimento

O LWC indexa Sources e páginas da Wiki nos níveis de documento, passagem e
frase. O agente pode começar com um contexto pequeno e relevante e expandir
somente o trecho exato de que precisa.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="Grafo de memória do LWC" width="100%"></p>

O grafo documental opcional conecta páginas, fontes, citações, links e relações
semânticas explícitas. O SQLite continua sendo a autoridade; Grafeo ou
SurrealDB fornece uma camada de navegação reconstruível. Cada relação preserva
motivo, proveniência, confiança e evidência.

### Conversão de documentos e leitura de Office

Os adaptadores opcionais Anydoc ou MarkItDown convertem arquivos locais
compatíveis em Markdown revisável antes da ingestão. O OfficeCLI oferece um
caminho separado, somente leitura e sujeito a consentimento para Word, Excel e
PowerPoint. Nenhum recurso é instalado ou ativado silenciosamente, e os arquivos
originais do Office não são modificados.

[Recuperação e indexação →](https://github.com/JanYork/llm-wiki-cli/wiki/Retrieval-and-Indexing) ·
[Grafo documental →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Knowledge-Graph) ·
[Conversão de documentos →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Conversion)

## Instalação

Para a maioria das pessoas, basta um comando:

    npm install --global @i-xor/lwc

Também há suporte para Homebrew, crates.io, releases do GitHub verificadas por
checksum e builds locais com Cargo.

[Instalação e atualizações →](https://github.com/JanYork/llm-wiki-cli/wiki/Installation-and-Upgrades)

## Skill complementar para agentes

A [Skill using-lwc](../../skills/using-lwc) incluída transforma o LWC em uma
camada de memória proativa. Ela recupera contexto limitado, separa conhecimento
de projeto e global, integra fontes, mantém citações e grava apenas conhecimento
verificado que vale a pena reutilizar.

Instale pelo [skills.sh](https://skills.sh/JanYork/llm-wiki-cli):

    npx skills add JanYork/llm-wiki-cli --skill using-lwc -g

A invocação canônica é <code>$using-lwc</code>. A Skill independe do agente e
inclui orientações específicas para memória, grafos documentais, Word Graph,
CodeGraph, tags fortes, conversão, configuração, recuperação e manutenção.

### Configuração nativa de agentes

O LWC detecta agentes compatíveis e configura as superfícies MCP, Skill, Hook e
Instructions disponíveis por meio de adaptadores AgentTarget idempotentes:

    lwc agent install --yes

O MCP unificado e somente leitura oferece memória Wiki limitada e contexto de
código opcional sem ampliar o workspace. Há suporte para Claude Code, Codex,
Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity,
GitHub Copilot in VS Code, Copilot CLI, Copilot for JetBrains e pi.

[Integração AgentTarget →](https://github.com/JanYork/llm-wiki-cli/wiki/AgentTarget-Installation-and-Integration)

## Início rápido

Normalmente, a pessoa descreve o objetivo e revisa o resultado; o agente opera
a CLI. O percurso completo está no
[guia de início rápido](https://github.com/JanYork/llm-wiki-cli/wiki/Quick-Start).

### 1. Inicializar uma Wiki de projeto

O agente cria uma Wiki local do projeto e define seu propósito e suas regras de
manutenção. O estado é excluído localmente do Git, a menos que o versionamento
seja uma escolha explícita.

### 2. Adicionar material de origem

Os arquivos selecionados se tornam snapshots imutáveis e sem duplicatas. O LWC
rastreia os caminhos e informa se o arquivo atual está inalterado, modificado,
ausente ou substituído.

### 3. Analisar e integrar uma fonte

O agente lê a fonte completa dentro de limites explícitos, escreve um resumo com
citações, atualiza o conhecimento compartilhado e só então conclui a ingestão.

### 4. Consultar a Wiki acumulada

A busca prioriza páginas mantidas sem perder o vínculo com a evidência. O agente
abre o texto original exato quando uma afirmação precisa ser verificada.

## Fluxo de trabalho do agente

O ciclo normal recupera conhecimento relevante, verifica fontes ou código atuais
quando a atualização importa, realiza a menor alteração comprovada e valida a
recuperação, os links e os grafos aplicáveis. Revisões amplas são publicadas
atomicamente em um changeset.

[Fluxo completo →](../../docs/agent-workflow.md)

## Alterações atômicas com vários comandos

Um changeset mantém uma atualização de várias etapas invisível até que seja
revisada e validada. O commit publica em uma transação apenas as entidades
afetadas, preserva trabalho não relacionado e falha com segurança quando há
conflito de revisão na mesma entidade.

Para operações compatíveis, um patch inverso exato permite rollback protegido
sem substituir toda a Wiki.

[Guia de changesets →](https://github.com/JanYork/llm-wiki-cli/wiki/Changesets)

## Escopos

| Escopo | Uso |
| --- | --- |
| project | Conhecimento pertencente à Wiki de projeto mais próxima |
| global | Conhecimento reutilizável entre projetos |
| all | Recuperação combinada somente leitura e Sync coordenado |

As gravações sempre apontam para um único armazenamento explícito; o LWC não
cria citações nem links implícitos entre projetos.

[Escopos e descoberta de projetos →](https://github.com/JanYork/llm-wiki-cli/wiki/Scopes-and-Project-Discovery)

## Busca e CJK

A busca é lexical, determinística e prioriza páginas mantidas. Título, caminho,
resumo, corpo, proveniência e evidência do grafo são pontuados separadamente;
há filtros por página, fonte e tipo e explicação exata da pontuação.

Para CJK, usa bigramas adjacentes e unigramas úteis; para texto latino, termos
alfanuméricos em minúsculas. Sem depender de dicionários, mantém estabilidade
com nomes de produtos, símbolos de código, texto multilíngue e vocabulário novo.

### Pesos e feedback explícitos

Pesos auditáveis expressam a importância duradoura de um documento. O feedback
de uma consulta só reordena candidatos correspondentes e armazena uma impressão
digital, não a consulta original. Nenhum dos dois faz conteúdo irrelevante
aparecer.

[Busca e contexto →](https://github.com/JanYork/llm-wiki-cli/wiki/Search-and-Context)

## Visualizador somente leitura e CodeGraph

O visualizador local apresenta páginas, fontes, Markdown, relações documentais
e estrutura do código por uma interface loopback limitada a GET/HEAD. Ele não
migra, atualiza nem constrói grafos.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="Inteligência de código do LWC CodeGraph" width="100%"></p>

O CodeGraph é exclusivo do projeto e inicializado explicitamente. Ele consulta
símbolos, chamadores, chamadas, dependências, arquivos e impacto, mantém a
telemetria desativada e atualiza o grafo atomicamente por arquivo proprietário.

O runtime reconhece TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust,
Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro,
Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig,
XML, .properties, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang e Terraform.

[Visualizador →](https://github.com/JanYork/llm-wiki-cli/wiki/Read-Only-Viewer) ·
[CodeGraph →](https://github.com/JanYork/llm-wiki-cli/wiki/Code-Graph)

## Manutenção e projeção

Lint, reindexação, materialização de Markdown, compactação, checkpoints e
projeção de grafos são operações explícitas. Trabalhos demorados são duráveis,
observáveis, retomáveis e aplicados em unidades documentais limitadas.

O SQLite permanece canônico. Índices, Markdown e grafos podem ser reconstruídos
sem reescrever o histórico das fontes nem o conhecimento atual da Wiki.

[Manutenção e diagnóstico →](https://github.com/JanYork/llm-wiki-cli/wiki/Maintenance-and-Diagnostics)

## Suíte de benchmarks

O benchmark opcional mede tempo de importação, latência de busca, Recall@5/10,
MRR e armazenamento em um corpus higienizado fornecido pelo usuário. Uma
comparação justa fixa máquina, corpus, consultas e condições e compara medianas
de várias execuções.

[Metodologia →](../../benchmarks/README.md)

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
