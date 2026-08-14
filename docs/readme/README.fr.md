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

**Compatible avec Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity et pi.**

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

Le modèle persistant comporte trois couches logiques :

| Couche | Contenu | Contrat |
| --- | --- | --- |
| Sources brutes | Instantanés immuables d’entrées sélectionnées | Ajouter via `source` ; ne jamais réécrire la vérité de la source. |
| Wiki | Pages, citations, liens et provenance entretenus par l’agent | Mettre à jour via `page` ; citer les sources et classer les connaissances durables hors source. |
| Schéma et finalité | Règles d’entretien et intention du projet | Guider chaque ingestion et révision ultérieure. |

SQLite est la référence canonique. L’arborescence Markdown est une projection reconstructible destinée aux humains et à des outils comme Obsidian. Les agents modifient les connaissances via `lwc`, sans éditer directement `.lwc/wiki.db` ni le Markdown projeté. Les commandes réussies renvoient du JSON sur stdout ; les erreurs, du JSON structuré sur stderr.

Les lectures laissent les stockages au format courant en lecture seule. Lorsqu’une nouvelle CLI ouvre pour la première fois un ancien stockage inscriptible, elle migre son schéma une fois, dans une transaction, avant de poursuivre la lecture.

## Rappel hiérarchique et graphe de connaissances

Chaque Source et page Wiki courante est indexée de manière déterministe en passages et phrases. SQLite reste l’autorité ; le FTS par spans et le graphe documentaire externe facultatif sont des index reconstructibles. La recherche existante continue de renvoyer des documents entiers, sauf granularité demandée.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="Graphe de mémoire de LWC" width="100%"></p>

```bash
lwc search "projection consistency" --granularity sentence --type page
lwc search "projection consistency" --granularity passage
lwc search "projection consistency" --granularity all --group-by document
lwc span get <SPAN_ID>
lwc span expand <SPAN_ID> --before 1 --after 1 --children 20
```

Les localisateurs de spans incluent l’empreinte du document et la version de segmentation. Si le corps a été remplacé, l’ancien localisateur échoue avec `stale_span` et fournit les métadonnées passées et actuelles ; LWC ne le remappe jamais silencieusement vers un texte similaire.

Pour explorer sans mots-clés, utilisez l’API de graphe typée et bornée :

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

Les arêtes automatiques se limitent aux faits structurels ou étayés. Les relations sémantiques doivent être explicites et auditables :

```bash
lwc graph relation set page:implementation DEPENDS_ON page:policy \
  --provenance source-grounded --source 12 \
  --reason "Source 12 states the required policy" --confidence 0.95
lwc graph relation list --from page:implementation
lwc graph relation retract page:implementation DEPENDS_ON page:policy \
  --reason "The dependency was superseded"
```

Les motifs de relation sont durables : n’y placez jamais d’identifiants, de secrets ni de chaîne de pensée brute.

Les documents SQLite restent la référence. Le stockage du graphe est désactivé par défaut ; activez exactement un moteur externe lorsque vous devez le parcourir. La configuration se superpose entre valeurs intégrées, globales et propres au projet :

```bash
lwc config show
lwc config set --graph grafeo
lwc config set --graph surrealdb
lwc config set --graph disabled
lwc config unset --graph
```

La conversion Markdown est une opération facultative distincte. `lwc init` affiche les mêmes indications lisibles par machine, sans installer ni activer de convertisseur. Installez un adaptateur, sélectionnez-le explicitement, convertissez vers un nouveau fichier Markdown local, relisez-le, puis ingérez-le :

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

La configuration accepte `--trans-timeout 1..900` et plusieurs `--trans-arg=<value>`. LWC exécute directement le binaire choisi, ne bascule jamais sur l’autre adaptateur, n’accepte que des fichiers locaux, limite entrée et sortie à 64 Mio et n’écrase aucune sortie existante. Gardez les identifiants dans l’environnement de l’adaptateur, pas dans la configuration LWC. Consultez la documentation officielle d’[Anydoc](https://github.com/firecrawl/anydoc) et de [MarkItDown](https://github.com/microsoft/markitdown) pour les formats et options.

Grafeo et SurrealDB embarqué utilisent des stockages auxiliaires jetables sous `.lwc/`. Chaque Work `graph-project` valide entièrement une Source/Page courante avec ses liens, citations et relations explicites avant de passer au document suivant. Mises à jour et suppressions ne mettent en file que les documents touchés ; reconstruction et reprise utilisent les mêmes unités. Les révisions historiques restent immuables et ne sont jamais retokenisées ni reprojetées. Suivez la progression avec `work list`, `work status` ou `work watch`, puis `work resume` après interruption. `graph status` indique le moteur et le nombre de documents projetés ; `graph verify` compare les clés courantes à SQLite.

## Installation

La plupart des utilisateurs devraient employer le prompt de configuration ci-dessus. Les commandes manuelles servent à la maintenance, au débogage ou aux environnements qui ne peuvent pas installer le Skill.

Homebrew (Bottles pour macOS Apple silicon et Linux x86_64) :

```bash
brew install JanYork/tap/lwc
```

npm (Node.js 22+) :

```bash
npm install --global @i-xor/lwc
```

crates.io :

```bash
cargo install --locked lwc
```

GitHub :

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | sh
```

L’installateur prend en charge macOS x86_64/aarch64, Linux glibc et Windows Git Bash, vérifie la somme de contrôle et installe ou met à jour `lwc`. Il utilise `~/.local/bin` par défaut ou met à jour une copie dans `~/.local/bin` ou `~/.cargo/bin`. Pour choisir un autre répertoire :

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | LWC_INSTALL_DIR="$HOME/bin" sh
```

Ou compilez depuis GitHub avec Cargo :

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

Ou installez depuis une copie locale du dépôt :

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## Skill compagnon pour agents

Le dépôt contient [`skills/using-lwc`](../../skills/using-lwc), un Agent Skill qui fait de `lwc` une couche de mémoire proactive pour les sessions substantielles. Installez-le depuis [skills.sh](https://skills.sh/JanYork/llm-wiki-cli) :

```bash
npx skills add JanYork/llm-wiki-cli --skill using-lwc -g
```

Vous pouvez aussi le copier depuis une copie locale du dépôt vers le dossier de Skills utilisateur de l’environnement d’exécution. Pour Codex :

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/using-lwc "$HOME/.agents/skills/"
```

L’invocation canonique est `$using-lwc`.

Une fois déclenché, le Skill :

- trouve une CLI compatible ou installe la version officielle vérifiée par somme de contrôle ;
- initialise une fois la mémoire globale dans `~/.lwc/` ;
- rappelle un contexte global et projet borné avant de répéter une enquête ;
- initialise le projet actif lors d’un appel explicite, sinon demande d’abord ;
- refuse les écritures de projet hors de la racine de workspace autorisée ;
- sépare les faits du projet des connaissances globales réutilisables ;
- intègre les sources et réécrit les réponses durables dans le Wiki.

`SKILL.md` est un routeur bref, pas un manuel monolithique. Il renvoie vers des documents ciblés sur la mémoire de base, le déclenchement, la mémoire active, le graphe physique, le Word Graph borné, CodeGraph, les strong tags, la conversion, l’intégration des agents et la reprise/maintenance. Chacun précise quand utiliser ou ignorer la fonction, le flux minimal, la limite de consentement et les preuves de fin.

Le Skill découvre normalement le projet depuis le dossier courant et appelle directement `lwc` installé globalement. `LWC_PROJECT_ROOT` délimite un projet explicitement ciblé ; ce n’est pas un préfixe à exporter pour les commandes quotidiennes du projet courant.

Définissez `LWC_AUTO_INSTALL=0` pour désactiver l’installation automatique. Celle-ci exécute l’installateur relu livré avec le Skill, fait confiance à ce dépôt et à son périmètre GitHub Releases, puis compare l’archive à `SHA256SUMS`. La somme protège l’intégrité ; ce n’est pas une signature de l’éditeur. Les binaires couvrent macOS x86_64/aarch64, Linux glibc et Windows via Git Bash. `SKILL.md` suit la disposition Agent Skills ; `agents/openai.yaml` fournit les métadonnées OpenAI/Codex. La CLI est indépendante de l’environnement d’exécution : tout agent capable de l’exécuter et de charger ou adapter les instructions peut utiliser LWC. Les commandes de Skill, instructions globales et Hooks restent propres à cet environnement ; le prompt détecte donc l’hôte actuel.

### Configuration native des agents

LWC détecte les agents pris en charge et installe un MCP LWC unifié en lecture seule. Les 12 AgentTargets enregistrés sont des adaptateurs complets : ils installent tous les points d’intégration officiels MCP, Skill, Hook et Instructions fondés sur des fichiers, pour chaque hôte et périmètre, et signalent explicitement ceux que l’interface gère, qui sont en préversion ou qui ne sont pas pris en charge.

```bash
lwc agent install --yes
lwc agent status --target all --location global
lwc agent install --print-config codex
lwc agent refresh --target codex,claude
lwc agent uninstall --target codex,claude --yes
```

`--yes` sélectionne les agents détectés, le périmètre global et les Hooks de cycle de vie/prompt par défaut. `--no-prompt-hook` omet le Hook par prompt de Claude. L’entrée installée est `lwc -> serve --mcp` ; l’unique outil `lwc_explore` lit par défaut une mémoire Wiki bornée et accepte les modes `code` et `all`. Le `projectPath` demandé doit rester dans le workspace où l’hôte MCP a lancé LWC. L’outil ne télécharge ni n’initialise CodeGraph. Installations et refresh répétés sont idempotents octet pour octet ; uninstall restaure seulement l’état possédé par LWC et conserve les index. Les paquets facultatifs Codex, Claude Code et Pi résident dans `integrations/`. Un paquet n’accorde ni ne contourne la confiance native. Ne combinez pas installateur direct et paquet natif pour le même agent. Chaque paquet embarque le Skill complet, sans gestionnaire tiers ni environnement propre au mainteneur.

Pi expose le MCP LWC via son pont d’extension officiel, car il n’intègre pas MCP. Les autres Targets n’enregistrent que `lwc serve --mcp` ; CodeGraph reste un plan interne de contexte et ne devient jamais un second MCP. Les réglages de confiance et permissions pilotés par l’interface restent gérés par l’utilisateur. Les points d’intégration en préversion sont signalés, et les périmètres partiels installent ce qui est pris en charge sans dégrader ni rejeter tout le Target. Les chemins globaux Kiro respectent `KIRO_HOME`.

L’interface Target, l’ordre du registre, les règles de détection et les chemins MCP suivent le modèle d’adaptateur de l’installateur CodeGraph sous licence MIT. LWC y ajoute le MCP unifié, le rapport par surface, Skills et Hooks, la propriété des fichiers partagés et un rollback exact. Voir [`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md).

La sortie `lwc init` et les Hooks de début/compactage exposent des faits `LWC_READINESS` bornés sur le Wiki, le graphe physique, le runtime et l’index CodeGraph, ainsi que les commandes d’intégration. L’état du graphe distingue consentement configuré et projection en attente ou en échec. La détection est en lecture seule et n’active ni n’initialise jamais de graphe. Lorsque les deux nécessitent une autorisation, la base portable est du texte simple :

```text
1. Enable physical document graph and CodeGraph (recommended)
2. Enable physical document graph only
3. Enable CodeGraph only
4. Later
```

Après un choix explicite `1`, l’agent initialise si besoin le Wiki, active Grafeo, attend et vérifie le Work de projection, initialise CodeGraph et contrôle séparément les deux résultats. `Later` ne change rien et ne bloque pas la tâche. Les plugins peuvent afficher les mêmes identifiants dans leur interface ; les cases à cocher ne sont jamais requises.

Les strong tags chargent intégralement quelques règles ou runbooks essentiels, de manière explicite et bornée :

```bash
lwc tag set "operations" incident-response --priority 100 --reason "primary runbook"
lwc load tag "operations" --limit 3
lwc tag autoload "operations" --enable --priority 100 --limit 3 \
  --max-chars 50000 --reason "required at session boundaries"
```

Ce n’est pas une recherche dérivée des tokens : limites et budgets de caractères s’appliquent avant l’entrée des pages complètes dans le contexte.

## Démarrage rapide

Cette section décrit le protocole CLI exécuté par l’agent. En usage normal, une personne n’a pas à lancer ces commandes.

### 1. Initialiser un Wiki de projet

```bash
cd your-project
lwc init
printf '# Schema\nEvery page declares provenance; source-grounded claims cite sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

L’initialisation ajoute si nécessaire `.lwc/` au `info/exclude` local de Git, sans modifier `.gitignore`. N’utilisez `lwc init --no-git-exclude` que si le Wiki doit volontairement être versionné.

### 2. Ajouter des sources

```bash
lwc source add-dir docs/
```

Les fichiers sans titre utilisent leur origine comme libellé stable et lisible. Les octets identiques sont dédupliqués par SHA-256. Les sources résolues hors de la racine active requièrent `--allow-external-source`. Les marqueurs d’identifiants à forte confiance sont refusés, sauf validation explicite après relecture avec `--acknowledge-sensitive-source`.

Chaque ajout enregistre aussi le chemin observé et son instantané immuable actuel. Avant de vous fier à une preuve issue d’un fichier, vérifiez seulement les sources pertinentes :

```bash
lwc source status 7 12
```

La commande transmet chaque fichier courant à SHA-256 et sépare la lignée du chemin (`current` ou `superseded`) de l’état du système (`current`, `modified`, `missing`, `unreadable`, `oversized` ou `unstable`). Elle est en lecture seule. `source status --all` coûte proportionnellement à tous les octets suivis ; réservez-le à la maintenance. Relisez un chemin modifié avant d’actualiser les connaissances :

```bash
lwc source diff 7
lwc source refs 7 --limit 1000
```

`source diff` compare la source immuable au fichier courant ou à un autre instantané avec `--to-source`. Le diff est borné à 8 Mio et 200 000 lignes par côté, 20 000 caractères Unicode par défaut et 100 000 avec `--max-chars`. Si une source a plusieurs chemins, choisissez-en un avec `--path`. Un diff tronqué n’est qu’un aperçu. `source refs` liste les candidats qui citent directement la source, sans prouver l’impact sémantique. Ne relancez `source add` qu’après avoir validé une révision significative. A -> B -> A conserve trois observations, même si A réutilise son source ID. Les chemins externes requièrent à nouveau `--allow-external-source` ; un texte signalé exige aussi `--acknowledge-sensitive-source` après relecture.

Les sources migrées d’anciens stockages restent explicitement non suivies, car LWC ne devine pas les chemins historiques. Réajoutez le fichier une fois pour créer sa première révision suivie. Si le fichier ou la révision la plus récente du chemin change pendant le contrôle, LWC renvoie `source_status_unstable` ; recommencez.

Pour un import atomique, les chemins d’un manifeste JSON sont résolus depuis son dossier :

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

### 3. Analyser et intégrer une source

```bash
lwc ingest next --context-limit 50 --source-max-chars 100000
lwc ingest analyze 1 --file analysis.md
```

Utilisez `lwc ingest claim 7` lorsqu’un manifeste ou ordonnanceur a déjà choisi un source ID en attente.

Si `source_window.has_more` vaut true, poursuivez depuis `source_window.next_offset_chars` :

```bash
lwc source show 1 --offset-chars 100000 --max-chars 100000
```

Avant de terminer, créez une page source-summary citée et intégrez l’apport à au moins une page non source :

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

Les deux couches sont obligatoires : la page source aide à la navigation et à la provenance ; la page partagée permet l’accumulation. Si une source ne change réellement aucune page partagée, terminez avec une justification précise et auditable :

```bash
lwc ingest complete 1 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

Les citations produisent automatiquement la provenance `source-grounded`. Pour une connaissance issue de l’utilisateur, d’une observation d’agent ou d’une hypothèse, répétez `--provenance` au lieu d’inventer une source :

```bash
lwc page put architecture-decision \
  --title "Architecture decision" \
  --kind query \
  --summary "Accepted constraint and remaining uncertainty" \
  --file decision.md \
  --provenance user-provided \
  --provenance hypothesis
```

`page put` remplace l’ensemble complet des citations et provenances explicites. Lisez d’abord la page et répétez chaque `--source` et `--provenance` encore valable. Ne passez pas `source-grounded`, dérivé des citations. La provenance apparaît dans page, context, search, refs et la projection, sans modifier le classement.

### 4. Interroger le Wiki accumulé

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc search "question keywords" --limit 20 --explain
lwc search "concept only" --type page --kind concept
lwc search "exact evidence" --type source
lwc page show source-1
```

## Flux de travail de l’agent

1. Collecter des sources immuables.
2. Réserver une tâche avec `lwc ingest next`, ou `ingest claim <ID>` si la source est choisie.
3. Lire toutes les fenêtres, le schéma, la finalité et le contexte borné.
4. Analyser avant de générer des pages.
5. Créer ou réviser résumé et pages partagées avec des citations `--source` explicites.
6. Terminer après les deux contrôles, ou consigner pourquoi aucune page ne doit changer.
7. Placer une ingestion multicommande ou une révision large dans un changeset, valider le brouillon et publier atomiquement.
8. Employer `search`, `context`, `graph` et `lint` pour maintenir la cohérence.

Voir [docs/agent-workflow.md](../../docs/agent-workflow.md) pour le contrat complet. `lwc --help` et `lwc <command> --help` détaillent préconditions, transitions, effets et actions suivantes.

## Modifications atomiques multicommandes

Une commande `source` ou `page` est transactionnelle. Utilisez un changeset lorsqu’une mise à jour logique exige plusieurs commandes sans exposer un Wiki partiel :

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

Les lectures du brouillon voient les écritures préparées ; SQLite et Markdown actifs restent inchangés. La base commence comme une petite surcouche sparse, sans copier ni checkpoint du Wiki actif. `changeset show` rapporte opérations, révisions et état de préparation sans lancer lint. Commit valide et applique seulement les entités touchées, préservant les écritures sans rapport ; un conflit sur la même entité échoue sans écraser aucun côté. Brouillons vides et erreurs lint sont refusés ; ni force ni fusion automatique. `--allow-lint-issues --reason "reviewed pre-existing debt"` est réservé à la dette auditée non introduite par le lot. Après commit, répétez les mêmes contrôles sur l’état actif. Commit fige le brouillon ; `changeset_frozen` bloque toute nouvelle écriture. Relancez le même commit pour reprendre ou abandonnez après conflit, sans ajouter de travail.

```bash
lwc --scope project changeset discard architecture-refresh
lwc --scope project changeset rollback <CHANGESET_ID>
```

Discard ne touche qu’un brouillon non validé. Commit écrit un patch inverse avec checksum limité aux entités touchées et renvoie l’ID exact ; rollback ne restaure que celles-ci et refuse si l’une a changé. Changesets project/global sont séparés, `--scope all` invalide, et `init`, `maintenance`, `checkpoint` ainsi que les changesets imbriqués refusent `--changeset`. Aucun second Markdown n’est projeté. Si une erreur indique `committed=true` avec cleanup ou matérialisation restante, ne répétez pas les modifications ; exécutez la reprise indiquée.

Le commit sparse possède des patchs exacts pour Source add/ingest, Page put/remove, schema, purpose et les recherches enregistrées. Poids et relations sémantiques échouent avant checkpoint, verrou ou modification avec `changeset_sparse_unsupported` ; appliquez-les directement, entité par entité, en attendant leurs patchs inverses.

## Périmètres

| Périmètre | Store | Usage |
| --- | --- | --- |
| `project` | `.lwc/wiki.db` de l’ancêtre le plus proche | Par défaut, connaissances du projet |
| `global` | `~/.lwc/wiki.db` | Connaissances réutilisables |
| `all` | project et global | Seulement `search` et `context` combinés |

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
```

Les écritures sont explicites. `all` ne crée ni citation ni lien entre stockages ; `search --record` ajoute seulement l’opération à chaque stockage sélectionné.

## Recherche et CJK

La recherche est lexicale et déterministe.

- Les termes sont du texte simple, pas de la syntaxe FTS brute.
- `--type auto` privilégie les pages compilées, masque leurs sources associées et garde les sources en repli.
- Choisissez `--type page`, `--type source` ou `--type all` ; répétez `--kind` pour filtrer.
- Les termes CJK utilisent des bigrammes adjacents ; les unigrammes utiles conservent les recherches d’un caractère.
- Le latin est découpé en tokens alphanumériques minuscules.
- Titre, nom de fichier, path/slug, résumé et corps sont évalués séparément ; titres et chemins reçoivent des bonus de score bornés.
- README, index, aperçus et documents centraux de navigation sont abaissés selon la requête ; demander le README supprime cette pénalité.
- Les candidats peuvent recevoir un bonus de lien direct ou de source partagée. Les voisins communs seuls ne changent pas l’ordre ; les documents de navigation trop généraux sont pénalisés.
- `--explain` renvoie le calcul exact des signaux lexicaux, génériques, graphes, poids manuels et feedback. Seul `--record` consigne la requête.
- Des coefficients fixes et des rangs « plus petit = meilleur » rendent project/global comparables sous `--scope all`.

Aucun dictionnaire de segmentation n’est requis, pour rester stable avec noms de produits, noms de code, termes mixtes et vocabulaire nouveau.

### Poids et feedback explicites

Utilisez un poids documentaire pour un jugement durable indépendant de la requête, et le feedback pour l’empreinte exacte de tokens ordonnés :

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

Les valeurs sont `-2`, `-1`, `1` et `2` ; `clear` vaut zéro. Les deux mécanismes ne font que réordonner les candidats lexicaux. `user-provided` prime sur `agent-observed`, les deux restant auditables. Le feedback conserve une empreinte SHA-256, pas la requête, et ne se transfère pas aux reformulations. Motifs et opérations sont durables : ne copiez pas de requête sensible dans `--reason`. Les mutations exigent `project` ou `global` ; `--scope all` est refusé.

## Visionneuse en lecture seule et CodeGraph

`lwc view` lance au premier plan un inspecteur limité à loopback et ouvre le navigateur. Il sert une seule application TS + Lit embarquée, sans CDN ni runtime Node à l’usage, et n’expose que des API GET/HEAD. Pages, sources, Markdown, graphe de connaissances et graphe de code facultatif sont lus depuis le projet sans migration, refresh ni construction :

```bash
lwc view
lwc view --port 4173 --no-open
```

La visionneuse démarre en anglais. Le contrôle `中文` / `EN` change la langue ; le navigateur mémorise le choix tandis que le Wiki reste dans sa langue d’origine. Les graphes utilisent une vue 3D inspirée d’Obsidian, avec petits nœuds, libellés persistants, liens fins, rotation et zoom.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="Intelligence de code LWC CodeGraph" width="100%"></p>

L’indexation du code est propre au projet et désactivée jusqu’à une initialisation explicite. Le fork CodeGraph fixé est téléchargé une fois depuis GitHub Releases, vérifié par SHA-256 et mis en cache dans `~/.lwc/runtime/codegraph/<PIN>/<TARGET>/` ; chaque projet ne garde que son index `.lwc/codegraph`. La télémétrie est toujours coupée et aucun état `.codegraph` n’est utilisé.

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

La version figée du runtime reconnaît les langages et formats liés au code suivants : TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust, Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro, Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig, XML, `.properties`, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang et Terraform. YAML, Twig et `.properties` sont suivis au niveau fichier ; les résolveurs de frameworks peuvent encore ajouter des relations. XML sert à extraire les mappers MyBatis.

`lwc cg` transmet toutes les requêtes CodeGraph. Les commandes globales de cycle de vie (`install`, `uninstall`, `upgrade`, `telemetry`, `daemon`, `daemons`) sont bloquées. Le pont `lwc cg serve --mcp` demeure pour l’ancienne compatibilité manuelle ; les nouvelles intégrations emploient `lwc serve --mcp`, qui réunit l’exploration bornée du Wiki et de CodeGraph derrière un outil en lecture seule. LWC maîtrise le runtime et impose la limite du projet. Écritures initiales, incrémentales, complètes, de mise à jour, suppression, résolution et reprise valident entièrement chaque fichier concerné avant le suivant ; le graphe courant reste lisible et les révisions historiques ne sont jamais mises à jour.

## Maintenance et projection

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

- Les commandes de maintenance renvoient immédiatement un `work` durable. Consultez `work status` ou attendez avec `work watch`, puis lisez `work.result`. La migration schema v10-v11 utilise le même mécanisme ; les commandes normales ne l’exécutent pas inline.
- `lint` est en lecture seule par défaut. Ajoutez `--record` seulement si ce contrôle doit entrer dans l’historique durable.
- `maintenance reindex` reconstruit les artefacts de recherche dérivés depuis SQLite.
- `maintenance materialize` reconstruit l’arbre Markdown projeté.
- `maintenance compact` tente seulement un checkpoint WAL truncate, sans optimisation FTS cachée. Exécutez-le lorsque le Wiki est inactif et vérifiez `busy` et `after_bytes`. Un processus de lecture occupé revient vite sans changer la référence.
- Les requêtes sont privées par défaut ; `--record` enregistre leur texte dans le journal durable.

`lwc checkpoint create <NAME>` utilise l’API de sauvegarde en ligne SQLite. Restaurez avec `lwc checkpoint restore <NAME>` ; LWC crée d’abord un checkpoint `pre-restore-*`, puis reconstruit la projection. `source remove <ID>` et `page remove <SLUG>` assurent une suppression protégée : les sources citées et pages ayant des liens entrants sont refusées. Supprimer la source courante d’un chemin suivi arrête son suivi sans exposer une ancienne révision comme actuelle.

Pour une ingestion multisource ou un remplacement large, préférez un changeset à un checkpoint manuel : commit écrit un patch inverse sparse, publie seulement les entités touchées dans une transaction et matérialise progressivement le Markdown. Il tente ensuite de tronquer le WAL ; `wal_checkpointed=false` indique un processus de lecture actif, pas l’échec du commit canonique.

Pour une sauvegarde externe, arrêtez les commandes `lwc` actives et copiez tout `.lwc/`. Ne copiez pas seulement `wiki.db` pendant qu’un processus d’écriture peut utiliser le WAL.

## Suite de benchmarks

Le benchmark facultatif importe un corpus UTF-8 local dans un Wiki temporaire et mesure l’import, les P50/P95 de recherche, Recall@5/10, MRR et le stockage avant/après compact. Le jeu de référence est un JSONL de requêtes et chemins attendus :

```bash
cargo build --release
LWC_BENCH_CORPUS=/path/to/sanitized-corpus \
LWC_BENCH_QUERY_SET=/path/to/query-set.jsonl \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
cargo test --test search_benchmark -- --ignored --nocapture
```

`cargo test --all-targets` couvre recherche page-first, filtres type/kind, fenêtres UTF-8, conditions d’ingestion, précision du graphe, migrations, lint et compactage WAL. Voir [benchmarks/README.md](../../benchmarks/README.md) pour le contrat et les comparaisons loyales.

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
