<h1 align="center">LWC — Mémoire proactive pour les agents d’IA</h1>

<p align="center"><strong>Piloté par les agents · Persistant · Adossé aux sources</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/@i-xor/lwc"><img alt="npm : @i-xor/lwc" src="https://img.shields.io/badge/npm-%40i--xor%2Flwc-CB3837?logo=npm"></a>
  <a href="https://crates.io/crates/lwc"><img alt="crates.io : lwc" src="https://img.shields.io/crates/v/lwc.svg"></a>
  <img alt="Node.js 22 ou version ultérieure" src="https://img.shields.io/badge/node-%3E%3D22-5FA04E?logo=nodedotjs">
  <img alt="Plateformes : macOS, Linux et Windows" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-666666">
  <a href="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://skills.sh/janyork/llm-wiki-cli/using-lwc"><img alt="skills.sh : using-lwc" src="https://img.shields.io/badge/skills.sh-using--lwc-000000?logo=vercel"></a>
  <a href="../../LICENSE"><img alt="Licence : Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="../../README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a> · <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> · <a href="README.fr.md">Français</a> ·
  <a href="README.ru.md">Русский</a>
</p>

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-social-preview.png" alt="LWC — Mémoire proactive pour les agents d’IA" width="100%"></p>

`lwc` est une CLI de mémoire proactive, pilotée par les agents et conçue pour les agents d’IA. Elle leur permet de retrouver, d’entretenir et de faire évoluer de façon autonome des connaissances persistantes et traçables jusqu’à leurs sources, d’une session à l’autre.

**Compatible avec Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity, GitHub Copilot in VS Code, Copilot CLI, Copilot for JetBrains et pi.**

LWC transforme des documents sélectionnés en Wiki durable. L’agent raisonne et synthétise ; `lwc` conserve les sources, pages, citations, liens, index et historiques afin que les connaissances s’accumulent au lieu d’être reconstituées à partir de fragments bruts à chaque requête.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-overview-en.png" alt="Vue d’ensemble de LWC" width="820"></p>

## LWC est une mémoire d’agent, pas un système RAG

RAG et LWC peuvent tous deux aider un LLM à exploiter des documents externes, mais ils ne conservent pas l’état au même endroit. Une requête RAG classique récupère des fragments bruts et produit une réponse ponctuelle :

```text
query -> retrieve chunks -> generate answer
```

LWC conserve le travail utile entre les requêtes :

```text
task -> recall maintained Wiki -> reason from sources and prior synthesis
     -> write durable improvements back
```

La recherche n’est qu’une opération de LWC, pas son principe d’organisation. L’artefact durable est un Wiki adossé aux sources dont les pages, citations, liens, contradictions et historiques évoluent avec les connaissances. LWC n’a donc besoin ni d’embeddings ni d’une base vectorielle, et ne jette pas chaque synthèse après la réponse. Il peut compléter un système RAG, mais n’est pas du RAG exécuté à chaque requête.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-source-grounding-en.png" alt="Sources et traçabilité dans LWC" width="820"></p>

### C’est l’agent qui pilote LWC

`lwc` est une interface machine destinée aux agents, pas une application de prise de notes pour humains. En usage normal, une personne sélectionne les sources, fixe les objectifs, pose les questions et relit les réponses ou le Markdown projeté. L’agent exécute la CLI, gère les périmètres, intègre les sources, entretient citations et liens, puis décide ce qui mérite d’être rappelé ou réécrit.

Ne pilotez pas manuellement le flux courant de `lwc`, sauf pour développer ou déboguer l’outil. Demandez plutôt à votre agent d’activer le Skill canonique `using-lwc`, généralement via `$using-lwc`.

## Recommandé : confiez la configuration de LWC à votre agent

Collez le prompt suivant dans l’agent que vous utilisez. Il installe la CLI globale, délègue la configuration des hôtes pris en charge à l’installateur AgentTarget idempotent de LWC et n’utilise la configuration native que pour un agent non enregistré.

<details>
<summary><strong>Copier le prompt de configuration complet</strong></summary>

```text
Configure entièrement LWC pour cet utilisateur. Exécute et vérifie le travail ;
ne te contente pas de décrire les commandes à lancer.

Sources de référence :
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

Exigences :
1. Lis ce README, `SECURITY.md` et `skills/using-lwc/SKILL.md`. Si `lwc` n’est
   pas disponible globalement, installe la version officielle vérifiée par somme
   de contrôle. Ne préfixe pas les commandes courantes avec un chemin privé vers
   le binaire ni avec `LWC_PROJECT_ROOT`.
2. Exécute `lwc --version`. Si la mémoire globale manque, initialise-la une seule
   fois avec `lwc --scope global init`, puis exécute `lwc agent install --yes`.
   Cette commande détecte les agents compatibles installés et configure en toute
   sécurité leurs MCP, Skill, Hook et Instructions aux emplacements officiels.
   Ne reproduis pas cette logique à la main et n’installe pas aussi un paquet
   natif pour le même agent.
3. Vérifie `lwc agent status --target all --location global`. Redémarre les agents
   concernés et effectue la validation de confiance habituelle des Hooks si
   nécessaire. N’initialise ni Wiki de projet ni graphe sans accord explicite
   pour ce projet.
4. Si l’environnement d’exécution actuel n’est pas un AgentTarget enregistré par LWC, suis ses
   conventions officielles au niveau utilisateur pour installer le Skill canonique
   `using-lwc`, un bloc d’instructions additif, `lwc serve --mcp` et un Hook de
   session borné, uniquement lorsque ces points d’intégration sont officiellement pris en
   charge. Préserve la configuration existante, reste idempotent et signale les
   points d’intégration non pris en charge au lieu d’inventer des chemins ou des clés.

Termine en indiquant la version de LWC, les Targets détectés et configurés, les
résultats de status, les fichiers modifiés, les points d’intégration non pris en charge et
toute action de redémarrage ou de confiance encore nécessaire.
```

</details>

## Origine et remerciements

`lwc` met en œuvre le modèle [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) proposé par Andrej Karpathy : un LLM construit et entretient progressivement un Wiki persistant et interconnecté, au lieu de reconstituer les connaissances depuis les documents bruts à chaque requête. L’architecture de la CLI et certains détails s’inspirent aussi de [`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki).

Le projet adapte ces idées en une CLI Rust pensée d’abord pour les agents et fondée sur SQLite.

## Conception fondamentale

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-architecture-en.png" alt="Architecture de LWC" width="100%"></p>

LWC sépare les connaissances durables en couches aux responsabilités claires :

| Couche | Rôle |
| --- | --- |
| Sources brutes | Instantanés immuables de preuves sélectionnées |
| Wiki | Pages, citations, liens et provenance maintenus par l’agent |
| Schéma et objectif | Règles du projet guidant la maintenance future |

SQLite constitue la source canonique. Markdown, les index plein texte et les
graphes facultatifs sont des projections reconstructibles. Les opérations
renvoient du JSON structuré pour faciliter l’audit et la reprise.

[Découvrir l’architecture →](https://github.com/JanYork/llm-wiki-cli/wiki/Architecture-Overview)

## Rappel hiérarchique et graphe de connaissances

LWC indexe les Sources et les pages Wiki aux niveaux du document, du passage et
de la phrase. L’agent peut commencer par un contexte réduit et pertinent, puis
développer uniquement l’extrait exact dont il a besoin.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="Graphe de mémoire de LWC" width="100%"></p>

Le graphe documentaire facultatif relie pages, sources, citations, liens et
relations sémantiques explicites. SQLite reste l’autorité ; Grafeo ou SurrealDB
fournit une couche de parcours reconstructible. Chaque relation conserve sa
raison, sa provenance, son niveau de confiance et ses preuves.

### Conversion de documents et lecture Office

Les adaptateurs facultatifs Anydoc ou MarkItDown convertissent les fichiers
locaux compatibles en Markdown révisable avant ingestion. OfficeCLI offre une
voie distincte, en lecture seule et soumise au consentement pour Word, Excel et
PowerPoint. Rien n’est installé ni activé silencieusement, et les fichiers
Office sources ne sont jamais modifiés.

[Rappel et indexation →](https://github.com/JanYork/llm-wiki-cli/wiki/Retrieval-and-Indexing) ·
[Graphe documentaire →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Knowledge-Graph) ·
[Conversion de documents →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Conversion)

## Installation

Pour la plupart des utilisateurs, une seule commande suffit :

    npm install --global @i-xor/lwc

Homebrew, crates.io, les versions GitHub vérifiées par somme de contrôle et les
compilations Cargo locales sont également pris en charge.

[Installation et mises à niveau →](https://github.com/JanYork/llm-wiki-cli/wiki/Installation-and-Upgrades)

## Skill compagnon pour agents

Le [Skill using-lwc](../../skills/using-lwc) inclus transforme LWC en couche de
mémoire proactive. Il rappelle un contexte borné, sépare les connaissances du
projet des connaissances globales, intègre les sources, maintient les citations
et ne conserve que les connaissances vérifiées qui méritent d’être réutilisées.

Installez-le depuis [skills.sh](https://skills.sh/JanYork/llm-wiki-cli) :

    npx skills add JanYork/llm-wiki-cli --skill using-lwc -g

L’invocation canonique est <code>$using-lwc</code>. Le Skill est indépendant de
l’agent et comprend des guides ciblés pour la mémoire, les graphes documentaires,
Word Graph, CodeGraph, les balises fortes, la conversion, la configuration, la
reprise et la maintenance.

### Configuration native des agents

LWC détecte les agents compatibles et configure leurs surfaces MCP, Skill, Hook
et Instructions disponibles au moyen d’adaptateurs AgentTarget idempotents :

    lwc agent install --yes

Le MCP unifié et en lecture seule fournit une mémoire Wiki bornée et un contexte
de code facultatif sans élargir l’espace de travail. Claude Code, Codex, Cursor,
OpenCode, Gemini CLI, Kiro, Hermes, Antigravity, GitHub Copilot in VS Code,
Copilot CLI, Copilot for JetBrains et pi sont pris en charge.

[Intégration AgentTarget →](https://github.com/JanYork/llm-wiki-cli/wiki/AgentTarget-Installation-and-Integration)

## Démarrage rapide

En usage normal, la personne décrit l’objectif et examine le résultat ; l’agent
pilote la CLI. Le parcours complet figure dans le
[guide de démarrage rapide](https://github.com/JanYork/llm-wiki-cli/wiki/Quick-Start).

### 1. Initialiser un Wiki de projet

L’agent crée un Wiki local au projet et définit son objectif et ses règles de
maintenance. Son état est exclu localement de Git, sauf décision explicite de le
versionner.

### 2. Ajouter des sources

Les fichiers sélectionnés deviennent des instantanés immuables et dédupliqués.
LWC suit leurs chemins et peut indiquer si le fichier actuel est inchangé,
modifié, absent ou remplacé.

### 3. Analyser et intégrer une source

L’agent lit la source complète dans des limites explicites, rédige un résumé
cité, actualise les connaissances partagées et ne termine l’ingestion qu’après
avoir rendu les deux couches cohérentes.

### 4. Interroger le Wiki accumulé

La recherche donne la priorité aux pages maintenues sans perdre le lien avec les
preuves. L’agent ouvre le texte source exact lorsqu’une affirmation doit être
vérifiée.

## Flux de travail de l’agent

Le cycle normal consiste à rappeler les connaissances pertinentes, vérifier les
sources ou le code actuels lorsque la fraîcheur importe, effectuer la plus
petite mise à jour vérifiée, puis valider le rappel, les liens et les graphes
applicables. Les révisions étendues sont publiées atomiquement dans un
changeset.

[Flux de travail complet →](../../docs/agent-workflow.md)

## Modifications atomiques multicommandes

Un changeset garde une mise à jour en plusieurs étapes invisible jusqu’à sa
révision et sa validation. Le commit publie dans une transaction uniquement les
entités touchées, préserve le travail sans rapport et échoue de façon sûre en
cas de conflit de révision sur la même entité.

Pour les opérations compatibles, un patch inverse exact autorise un rollback
protégé sans remplacer l’ensemble du Wiki.

[Guide des changesets →](https://github.com/JanYork/llm-wiki-cli/wiki/Changesets)

## Périmètres

| Périmètre | Usage |
| --- | --- |
| project | Connaissances appartenant au Wiki de projet le plus proche |
| global | Connaissances réutilisables entre projets |
| all | Rappel combiné en lecture seule et Sync coordonné |

Les écritures ciblent toujours un seul stockage explicite ; LWC ne crée ni
citation ni lien implicite entre projets.

[Périmètres et découverte des projets →](https://github.com/JanYork/llm-wiki-cli/wiki/Scopes-and-Project-Discovery)

## Recherche et CJK

La recherche est lexicale, déterministe et privilégie les pages maintenues. Le
titre, le chemin, le résumé, le corps, la provenance et les preuves du graphe
sont évalués séparément ; des filtres de page, source et type ainsi qu’une
explication exacte du score sont disponibles.

Le texte CJK utilise des bigrammes adjacents et des unigrammes utiles ; le texte
latin emploie des termes alphanumériques en minuscules. Sans dictionnaire, le
comportement reste stable pour les noms de produits, les symboles de code, le
texte multilingue et le vocabulaire émergent.

### Poids et feedback explicites

Des poids auditables expriment l’importance durable d’un document. Le feedback
propre à une requête ne reclasse que les candidats correspondants et conserve
une empreinte, pas la requête brute. Aucun des deux ne peut faire apparaître un
contenu sans rapport.

[Recherche et contexte →](https://github.com/JanYork/llm-wiki-cli/wiki/Search-and-Context)

## Visionneuse en lecture seule et CodeGraph

La visionneuse locale présente pages, sources, Markdown, relations documentaires
et structure du code via une interface loopback limitée à GET/HEAD. Elle
n’effectue ni migration, ni actualisation, ni construction de graphe.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="Intelligence de code LWC CodeGraph" width="100%"></p>

CodeGraph est propre au projet et s’initialise explicitement. Il interroge les
symboles, appelants, appelés, dépendances, fichiers et impacts, conserve la
télémétrie désactivée et met à jour le graphe atomiquement par fichier
propriétaire.

Le runtime reconnaît TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust,
Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro,
Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig,
XML, .properties, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang et Terraform.

[Visionneuse →](https://github.com/JanYork/llm-wiki-cli/wiki/Read-Only-Viewer) ·
[CodeGraph →](https://github.com/JanYork/llm-wiki-cli/wiki/Code-Graph)

## Maintenance et projection

Lint, réindexation, matérialisation Markdown, compactage, checkpoints et
projection des graphes sont des opérations explicites. Les tâches longues sont
durables, observables, reprenables et exécutées par unités documentaires bornées.

SQLite reste canonique. Les index, Markdown et graphes peuvent être reconstruits
sans réécrire l’historique des sources ni les connaissances actuelles du Wiki.

[Maintenance et diagnostic →](https://github.com/JanYork/llm-wiki-cli/wiki/Maintenance-and-Diagnostics)

## Suite de benchmarks

Le benchmark facultatif mesure le temps d’importation, la latence de recherche,
Recall@5/10, MRR et le stockage sur un corpus assaini fourni par l’utilisateur.
Une comparaison équitable fixe la machine, le corpus, les requêtes et les
conditions, puis compare les médianes de plusieurs exécutions.

[Méthodologie →](../../benchmarks/README.md)

## Limites et non-objectifs

Contraintes actuelles :

- base de connaissances pour une machine et un utilisateur ;
- flux de texte UTF-8 ;
- limite de 64 Mio par schema, purpose, source ou corps de page ;
- recherche lexicale, pas de récupération vectorielle sémantique.

Non-objectifs délibérés :

- aucun appel LLM intégré ;
- aucune base vectorielle ;
- aucun daemon ni service d’arrière-plan ;
- aucune interface Web ou bureau ;
- aucun contrat d’édition directe de la base.

Si la projection Markdown dérive, reconstruisez-la. Si le schéma SQLite est erroné, corrigez-le par la CLI et les migrations, pas à la main.

## Contribuer

Issues et pull requests sont bienvenus, notamment sur :

- l’ergonomie du flux d’agents ;
- la projection déterministe ;
- les contrats durables de citation et d’entretien des pages ;
- la qualité de recherche dans les corpus techniques multilingues.

Lisez [CONTRIBUTING.md](../../CONTRIBUTING.md) avant d’ouvrir une pull request. Signalez les problèmes de sécurité selon [SECURITY.md](../../SECURITY.md).

## Licence

Sous [licence Apache 2.0](../../LICENSE).
