<h1 align="center">LWC — Memoria proactiva para agentes de IA</h1>

<p align="center"><strong>Dirigido por agentes · Persistente · Basado en fuentes</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/@i-xor/lwc"><img alt="npm: @i-xor/lwc" src="https://img.shields.io/badge/npm-%40i--xor%2Flwc-CB3837?logo=npm"></a>
  <a href="https://crates.io/crates/lwc"><img alt="crates.io: lwc" src="https://img.shields.io/crates/v/lwc.svg"></a>
  <img alt="Node.js 22 o posterior" src="https://img.shields.io/badge/node-%3E%3D22-5FA04E?logo=nodedotjs">
  <img alt="Plataformas: macOS, Linux y Windows" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-666666">
  <a href="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://skills.sh/janyork/llm-wiki-cli/using-lwc"><img alt="skills.sh: using-lwc" src="https://img.shields.io/badge/skills.sh-using--lwc-000000?logo=vercel"></a>
  <a href="../../LICENSE"><img alt="Licencia: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="../../README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a> · <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> · <a href="README.fr.md">Français</a> ·
  <a href="README.ru.md">Русский</a>
</p>

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-social-preview.png" alt="LWC — Memoria proactiva para agentes de IA" width="100%"></p>

`lwc` es una CLI de memoria proactiva, dirigida por agentes y pensada para agentes de IA. Permite que recuperen, mantengan y hagan evolucionar por sí mismos conocimiento persistente y trazable hasta sus fuentes entre una sesión y la siguiente.

**Funciona con Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity y pi.**

LWC convierte documentos seleccionados en una Wiki duradera. El agente razona y sintetiza; `lwc` conserva fuentes, páginas, citas, enlaces, índices e historial para que el conocimiento se acumule, en vez de reconstruirse desde fragmentos sin procesar en cada consulta.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-overview-en.png" alt="Vista general de LWC" width="820"></p>

## LWC es memoria para agentes, no RAG

RAG y LWC pueden ayudar a un LLM a trabajar con documentos externos, pero conservan el estado en lugares distintos. Una petición RAG típica recupera fragmentos sin procesar y genera una respuesta puntual:

```text
query -> retrieve chunks -> generate answer
```

LWC conserva el trabajo útil entre peticiones:

```text
task -> recall maintained Wiki -> reason from sources and prior synthesis
     -> write durable improvements back
```

La recuperación es una operación de LWC, no su principio organizador. El artefacto duradero es una Wiki basada en fuentes cuyas páginas, citas, enlaces, contradicciones e historial se revisan a medida que cambia el conocimiento. Por eso LWC no necesita embeddings ni una base de datos vectorial, y tampoco desecha cada síntesis al terminar una respuesta. Puede complementar a RAG, pero no es RAG ejecutado en cada consulta.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-source-grounding-en.png" alt="Fuentes y trazabilidad en LWC" width="820"></p>

### El agente opera LWC

`lwc` es una interfaz de máquina para agentes, no una aplicación de notas orientada a personas. En el uso normal, una persona selecciona fuentes, fija objetivos, plantea preguntas y revisa las respuestas o el Markdown proyectado. El agente ejecuta la CLI, gestiona los ámbitos, integra fuentes, mantiene citas y enlaces y decide qué merece recuperarse o escribirse de vuelta.

No dirijas manualmente el flujo habitual de `lwc` salvo que estés desarrollando o depurando la herramienta. Pide a tu agente que active el Skill canónico `using-lwc`, normalmente mediante `$using-lwc`.

## Recomendado: pide a tu agente que configure LWC

Pega este prompt en el agente que utilizas. Instala la CLI global, delega toda configuración de hosts compatibles al instalador idempotente AgentTarget de LWC y solo recurre a la configuración nativa cuando el agente aún no está registrado.

<details>
<summary><strong>Copiar el prompt de configuración completo</strong></summary>

```text
Configura LWC por completo para este usuario. Ejecuta y verifica el trabajo; no te
limites a describir los comandos que debo ejecutar.

Fuentes de referencia:
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

Requisitos:
1. Lee este README, `SECURITY.md` y `skills/using-lwc/SKILL.md`. Si `lwc` no se
   puede invocar globalmente, instala la versión oficial verificada por checksum;
   no antepongas una ruta privada al binario ni `LWC_PROJECT_ROOT` a los comandos
   habituales.
2. Ejecuta `lwc --version`; si falta la memoria global, inicialízala una sola vez
   con `lwc --scope global init`; después ejecuta `lwc agent install --yes`. Ese
   comando detecta los agentes compatibles instalados e instala de forma segura
   su MCP, Skill, Hook e Instructions en las ubicaciones oficiales. No reproduzcas
   esa lógica manualmente ni instales además un paquete nativo para el mismo agente.
3. Revisa `lwc agent status --target all --location global`. Reinicia los agentes
   afectados y completa la revisión de confianza habitual para los Hooks cuando
   corresponda. No inicialices una Wiki de proyecto ni ninguno de los grafos sin
   consentimiento explícito para ese proyecto.
4. Si el entorno de ejecución actual no es uno de los AgentTargets registrados por LWC, usa sus
   convenciones oficiales de usuario para instalar el Skill canónico `using-lwc`,
   un bloque de instrucciones aditivo, `lwc serve --mcp` y un Hook de sesión acotado,
   solo donde esos puntos de integración tengan soporte oficial. Conserva la configuración
   existente, mantén la idempotencia e informa de los puntos no compatibles
   en lugar de inventar rutas o claves.

Al terminar, informa de la versión de LWC, los Targets detectados y configurados,
los resultados de status, los archivos modificados, los puntos de integración no compatibles
y cualquier reinicio o acción de confianza que quede pendiente.
```

</details>

## Origen y agradecimientos

`lwc` implementa el patrón [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) propuesto por Andrej Karpathy: un LLM construye y mantiene de forma incremental una Wiki persistente e interconectada, en lugar de reconstruir el conocimiento desde documentos sin procesar en cada consulta. La arquitectura de la CLI y algunos detalles de implementación también se inspiran en [`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki).

Este proyecto adapta esas ideas a una CLI en Rust, orientada primero a agentes y respaldada por SQLite.

## Diseño fundamental

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-architecture-en.png" alt="Arquitectura de LWC" width="100%"></p>

El modelo de conocimiento persistente tiene tres capas lógicas:

| Capa | Contenido | Contrato |
| --- | --- | --- |
| Fuentes sin procesar | Instantáneas inmutables de entradas seleccionadas | Se añaden mediante `source`; la verdad de la fuente nunca se reescribe. |
| Wiki | Páginas, citas, enlaces y procedencia mantenidos por el agente | Se actualiza mediante `page`; se citan las fuentes y se clasifica el conocimiento duradero que no procede de ellas. |
| Esquema y propósito | Reglas de mantenimiento e intención del proyecto | Guían cada ingesta y revisión posterior. |

SQLite es la fuente canónica. El árbol Markdown es una proyección reconstruible para personas y herramientas como Obsidian. Los agentes modifican el conocimiento mediante `lwc`, no editando directamente `.lwc/wiki.db` ni el Markdown proyectado. Los comandos correctos devuelven JSON por stdout; los errores devuelven JSON estructurado por stderr.

Las lecturas mantienen en modo de solo lectura los almacenes con el formato actual. Cuando una CLI nueva abre por primera vez un almacén antiguo escribible, migra su esquema una vez y dentro de una transacción antes de continuar la lectura.

## Recuperación jerárquica y grafo de conocimiento

Cada Source y página Wiki vigente se indexa de forma determinista como pasajes y oraciones. SQLite sigue siendo la autoridad; el FTS por spans y el grafo documental externo opcional son índices reconstruibles. Las búsquedas existentes siguen devolviendo documentos completos salvo que se solicite otra granularidad.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="Grafo de memoria de LWC" width="100%"></p>

```bash
lwc search "projection consistency" --granularity sentence --type page
lwc search "projection consistency" --granularity passage
lwc search "projection consistency" --granularity all --group-by document
lwc span get <SPAN_ID>
lwc span expand <SPAN_ID> --before 1 --after 1 --children 20
```

Los localizadores de spans incluyen la huella del documento y la versión de segmentación. Un localizador cuyo cuerpo se haya sustituido falla con `stale_span` y muestra los metadatos anterior y actual; LWC nunca lo reasigna silenciosamente a un texto parecido.

Para explorar sin palabras clave, usa la API de grafos tipada y acotada:

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

Las aristas automáticas se limitan a hechos estructurales o respaldados por evidencias. Las relaciones semánticas deben ser explícitas y auditables:

```bash
lwc graph relation set page:implementation DEPENDS_ON page:policy \
  --provenance source-grounded --source 12 \
  --reason "Source 12 states the required policy" --confidence 0.95
lwc graph relation list --from page:implementation
lwc graph relation retract page:implementation DEPENDS_ON page:policy \
  --reason "The dependency was superseded"
```

Los motivos de una relación son contenido duradero: nunca incluyas credenciales, secretos ni cadenas de pensamiento sin procesar.

Los documentos de SQLite siguen siendo la autoridad. El almacenamiento del grafo está desactivado de forma predeterminada; habilita exactamente un motor externo cuando necesites recorrerlo. La configuración se compone por capas: valores integrados, globales y del proyecto.

```bash
lwc config show
lwc config set --graph grafeo
lwc config set --graph surrealdb
lwc config set --graph disabled
lwc config unset --graph
```

La conversión a Markdown es una operación opcional independiente. `lwc init` muestra la misma guía legible por máquina, pero nunca instala ni habilita un conversor. Instala un adaptador, selecciónalo expresamente, convierte la entrada a un archivo Markdown local nuevo, revísalo y solo entonces ingiérelo:

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

La configuración admite `--trans-timeout 1..900` y varias opciones `--trans-arg=<value>` para el adaptador elegido. LWC ejecuta directamente el binario fijado, nunca cambia al otro adaptador, solo acepta archivos locales, limita entrada y salida a 64 MiB y nunca sobrescribe una salida existente. Guarda las credenciales en el entorno del adaptador, no en la configuración de LWC. Consulta la documentación oficial de [Anydoc](https://github.com/firecrawl/anydoc) y [MarkItDown](https://github.com/microsoft/markitdown) para conocer formatos y opciones.

Grafeo y SurrealDB embebido usan almacenes auxiliares desechables en `.lwc/`. Cada Work `graph-project` confirma una Source/Page vigente y sus enlaces, citas y relaciones explícitas antes de empezar el siguiente documento. Las actualizaciones y eliminaciones solo ponen en cola los documentos afectados; reconstrucción y reanudación usan las mismas unidades. Las revisiones históricas de fuentes permanecen inmutables y nunca se tokenizan ni proyectan de nuevo. Consulta el progreso con `work list`, `work status` o `work watch`, y usa `work resume` tras una interrupción. `graph status` informa del motor y el número de documentos proyectados; `graph verify` compara sus claves vigentes con SQLite.

## Instalación

La mayoría de usuarios debería utilizar el prompt de configuración anterior. Los comandos manuales son para mantenimiento, depuración o entornos de agentes que no pueden instalar el Skill complementario.

Instalar con Homebrew (hay Bottles precompiladas para macOS con Apple silicon y Linux x86_64):

```bash
brew install JanYork/tap/lwc
```

Instalar con npm (Node.js 22+):

```bash
npm install --global @i-xor/lwc
```

Instalar desde crates.io:

```bash
cargo install --locked lwc
```

Instalar desde GitHub:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | sh
```

El instalador admite macOS x86_64/aarch64, Linux con glibc y Windows Git Bash; verifica el checksum de la versión e instala o actualiza `lwc`. Usa `~/.local/bin` de forma predeterminada, o actualiza una copia existente en `~/.local/bin` o `~/.cargo/bin`. Para elegir otro directorio:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | LWC_INSTALL_DIR="$HOME/bin" sh
```

También puedes compilar e instalar desde GitHub con Cargo:

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

O instalar desde una copia local del repositorio:

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## Skill complementario para agentes

El repositorio incluye [`skills/using-lwc`](../../skills/using-lwc), un Agent Skill que convierte `lwc` en una capa de memoria proactiva para sesiones sustanciales. Instálalo desde [skills.sh](https://skills.sh/JanYork/llm-wiki-cli):

```bash
npx skills add JanYork/llm-wiki-cli --skill using-lwc -g
```

También puedes copiarlo desde una copia local del repositorio al directorio de Skills de usuario del entorno de ejecución actual. Para Codex:

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/using-lwc "$HOME/.agents/skills/"
```

La invocación canónica es `$using-lwc`.

Cuando se activa, el Skill:

- encuentra una CLI compatible o instala la versión oficial verificada por checksum;
- inicializa una sola vez la memoria global en `~/.lwc/`;
- recupera contexto global y de proyecto acotado antes de repetir una investigación;
- inicializa el proyecto activo cuando se invoca expresamente y, en caso contrario, pregunta primero;
- rechaza escrituras de proyecto fuera de la raíz autorizada del workspace actual;
- separa los hechos del proyecto del conocimiento global reutilizable;
- integra fuentes y escribe respuestas duraderas de vuelta en la Wiki.

`SKILL.md` es un enrutador breve, no un manual monolítico. Enlaza documentos específicos sobre memoria básica, criterios de activación, memoria activa, grafo físico de documentos, Word Graph acotado, CodeGraph, etiquetas fuertes, conversión de documentos, incorporación de agentes y recuperación/mantenimiento. Cada documento indica cuándo usar u omitir la capacidad, su flujo mínimo, el límite de consentimiento y la evidencia de finalización.

El Skill suele descubrir el proyecto activo desde el directorio actual e invoca directamente el comando `lwc` instalado globalmente. `LWC_PROJECT_ROOT` es un límite explícito para un proyecto elegido deliberadamente, no un prefijo que deba exportarse en los comandos cotidianos del proyecto actual.

Define `LWC_AUTO_INSTALL=0` para desactivar la instalación automática. La instalación automática ejecuta el instalador revisado incluido en el Skill, confía en este repositorio y en su perímetro de publicación de GitHub Releases y compara el archivo descargado con `SHA256SUMS`; el checksum protege la integridad, no es una firma del editor. Los binarios cubren macOS x86_64/aarch64, Linux glibc y Windows mediante Git Bash. `SKILL.md` sigue la estructura de recursos de Agent Skills y `agents/openai.yaml` aporta metadatos para OpenAI/Codex. La CLI es independiente del entorno de ejecución: cualquier agente capaz de ejecutarla y cargar o adaptar las instrucciones del Skill puede usar LWC. Los comandos del Skill, las instrucciones globales y los Hooks dependen del entorno de ejecución, por lo que el prompt de configuración detecta y configura el host actual.

### Configuración nativa de agentes

LWC detecta agentes compatibles e instala un único MCP de LWC de solo lectura. Los 12 AgentTargets registrados son adaptadores completos: instalan todos los puntos de integración oficiales de MCP, Skill, Hook e Instructions basados en archivos disponibles para cada host y ámbito, y señalan de forma expresa los que gestiona la interfaz, están en vista previa o no son compatibles.

```bash
lwc agent install --yes
lwc agent status --target all --location global
lwc agent install --print-config codex
lwc agent refresh --target codex,claude
lwc agent uninstall --target codex,claude --yes
```

`--yes` selecciona los agentes detectados, el ámbito global y los Hooks predeterminados de ciclo de vida y prompt de cada Target. Usa `--no-prompt-hook` para omitir el Hook por prompt de Claude. La entrada instalada es `lwc -> serve --mcp`; su única herramienta, `lwc_explore`, consulta por defecto memoria Wiki acotada y admite los modos explícitos `code` y `all`. El `projectPath` solicitado debe permanecer dentro del workspace donde el host MCP inició LWC. La herramienta nunca descarga ni inicializa CodeGraph. Las instalaciones y actualizaciones repetidas son idempotentes byte a byte; la desinstalación solo restaura el estado propiedad de LWC y conserva los índices del proyecto. Los paquetes opcionales para Codex, Claude Code y Pi están en `integrations/`. Instalar un paquete no concede ni evita la confianza nativa. No combines el instalador directo y el paquete nativo para un mismo agente. Cada paquete incluye el Skill `using-lwc` completo, sin depender de un gestor de Skills externo ni del entorno particular del mantenedor.

Pi expone el MCP de LWC mediante su puente de extensiones oficial porque no incorpora MCP. Los demás Targets solo registran `lwc serve --mcp`; CodeGraph permanece como plano interno de contexto de código y nunca se registra como segundo MCP. Las opciones de confianza y permisos controladas por la interfaz siguen en manos del usuario. Los puntos de integración en vista previa se identifican como tales, y los ámbitos de proyecto parciales instalan lo que sí está soportado, sin degradar ni rechazar el Target completo. Las rutas globales de Kiro respetan `KIRO_HOME`.

La interfaz de Target, el orden del registro, las reglas de detección y las rutas MCP siguen el diseño del adaptador del instalador de CodeGraph, con licencia MIT. LWC añade el MCP unificado, informes por superficie, Skills y Hooks, propiedad de archivos compartidos y rollback exacto. Consulta [`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md).

La salida de `lwc init` en proyectos nuevos y los Hooks de inicio/compactación exponen hechos `LWC_READINESS` acotados sobre la Wiki, el grafo físico, el runtime y el índice de CodeGraph, además de los comandos de integración. La preparación del grafo físico distingue el consentimiento configurado de una proyección pendiente o fallida. La detección es de solo lectura y nunca habilita ni inicializa grafos. Cuando ambos grafos necesitan autorización, la base portable es texto sencillo para que los agentes sin checkboxes se comporten igual:

```text
1. Enable physical document graph and CodeGraph (recommended)
2. Enable physical document graph only
3. Enable CodeGraph only
4. Later
```

Tras elegir expresamente `1`, el agente inicializa una Wiki si falta, habilita Grafeo, espera y verifica su Work de proyección, inicializa CodeGraph y comprueba ambos resultados por separado. `Later` no cambia nada ni bloquea la tarea principal. Los plugins nativos pueden representar los mismos identificadores con su propia interfaz, pero los checkboxes nunca son obligatorios.

Las etiquetas fuertes permiten cargar páginas completas, de forma explícita y acotada, para reglas y runbooks esenciales:

```bash
lwc tag set "operations" incident-response --priority 100 --reason "primary runbook"
lwc load tag "operations" --limit 3
lwc tag autoload "operations" --enable --priority 100 --limit 3 \
  --max-chars 50000 --reason "required at session boundaries"
```

No es una búsqueda derivada de tokens: los límites y presupuestos de caracteres se aplican antes de introducir páginas completas en el contexto del agente.

## Inicio rápido

Esta sección documenta el protocolo CLI que ejecuta el agente. En el uso normal, una persona no necesita ejecutar estos comandos.

### 1. Inicializar una Wiki de proyecto

```bash
cd your-project
lwc init
printf '# Schema\nEvery page declares provenance; source-grounded claims cite sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

La inicialización añade, cuando hace falta, la ruta relativa `.lwc/` al archivo local `info/exclude` de Git sin modificar `.gitignore`. Usa `lwc init --no-git-exclude` solo si quieres versionar la Wiki expresamente.

### 2. Añadir material fuente

```bash
lwc source add-dir docs/
```

Los archivos sin título explícito usan su origen como alternativa estable y legible. Los bytes idénticos se deduplican por SHA-256. Las fuentes de proyecto que se resuelven fuera de la raíz activa de la Wiki requieren `--allow-external-source`. Los marcadores de credenciales de alta confianza se rechazan salvo que la fuente revisada se reconozca con `--acknowledge-sensitive-source`.

Cada adición correcta registra también la ruta observada y su instantánea inmutable actual. Antes de confiar en evidencia respaldada por archivos, comprueba solo las fuentes pertinentes:

```bash
lwc source status 7 12
```

El comando transmite cada archivo actual por SHA-256 e informa por separado del linaje de ruta (`current` o `superseded`) y del estado del sistema de archivos (`current`, `modified`, `missing`, `unreadable`, `oversized` o `unstable`). Es de solo lectura. Usa `source status --all` solo para mantenimiento explícito: su coste es proporcional a todos los bytes rastreados. Revisa una ruta modificada antes de actualizar conocimiento:

```bash
lwc source diff 7
lwc source refs 7 --limit 1000
```

`source diff` compara la fuente inmutable con su archivo actual, o con otra instantánea mediante `--to-source`. Devuelve un diff unificado acotado: hasta 8 MiB y 200.000 líneas por lado, 20.000 caracteres Unicode de forma predeterminada y 100.000 con `--max-chars`. Si una fuente se observó en varias rutas, elige una con `--path`. Un diff truncado solo es una vista previa. `source refs` enumera candidatos que citan directamente la fuente; no demuestra qué páginas están afectadas semánticamente. Vuelve a ejecutar `source add` solo tras revisar una revisión nueva y significativa. Una secuencia A -> B -> A conserva tres observaciones aunque A reutilice su source ID original. Las rutas externas requieren de nuevo `--allow-external-source`; el texto actual señalado también requiere `--acknowledge-sensitive-source` después de revisarlo.

Las fuentes migradas desde almacenes antiguos siguen expresamente sin rastrear porque LWC no adivina rutas históricas. Vuelve a añadir el archivo una vez para establecer su primera revisión. Si el archivo o la cabeza de ruta cambia durante la comprobación, LWC devuelve `source_status_unstable`; repite la operación en vez de confiar en un resultado de tiempos mezclados.

Para una importación atómica y seleccionada, las rutas de un manifiesto JSON se resuelven desde su directorio:

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

### 3. Analizar e integrar una fuente

```bash
lwc ingest next --context-limit 50 --source-max-chars 100000
lwc ingest analyze 1 --file analysis.md
```

Usa `lwc ingest claim 7` cuando un manifiesto o programador ya haya seleccionado un source ID pendiente.

Si `source_window.has_more` es true, continúa desde `source_window.next_offset_chars`:

```bash
lwc source show 1 --offset-chars 100000 --max-chars 100000
```

Antes de completar la ingesta, crea una página resumen con citas e integra su aportación en al menos una página que no sea de tipo source:

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

Ambas capas son obligatorias: la página source ayuda a navegar y acreditar la procedencia; la página compartida hace que el conocimiento se acumule. Si una fuente realmente no cambia ninguna página compartida, complétala con una explicación concreta y auditable:

```bash
lwc ingest complete 1 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

Las citas de fuentes exponen automáticamente la procedencia `source-grounded`. Para conocimiento duradero procedente del usuario, de una observación del agente o de una hipótesis explícita, repite `--provenance` según sea necesario en vez de inventar una fuente:

```bash
lwc page put architecture-decision \
  --title "Architecture decision" \
  --kind query \
  --summary "Accepted constraint and remaining uncertainty" \
  --file decision.md \
  --provenance user-provided \
  --provenance hypothesis
```

`page put` sustituye los conjuntos completos de citas y procedencia explícita. Lee primero la página existente y vuelve a especificar cada `--source` y cada valor `--provenance` no procedente de fuentes que siga siendo válido. No pases `source-grounded`: se deriva de las citas. La procedencia aparece en lecturas de página, context, search, referencias y proyección Markdown, pero no cambia el ranking.

### 4. Consultar la Wiki acumulada

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc search "question keywords" --limit 20 --explain
lwc search "concept only" --type page --kind concept
lwc search "exact evidence" --type source
lwc page show source-1
```

## Flujo de trabajo del agente

El flujo previsto es:

1. Reunir fuentes inmutables.
2. Reclamar una tarea con `lwc ingest next` de forma acotada, o usar `ingest claim <ID>` si la fuente ya está elegida.
3. Leer todas las ventanas devueltas, además del esquema, el propósito y el contexto acotado.
4. Analizar antes de generar páginas.
5. Crear o revisar un resumen y páginas duraderas compartidas con citas `--source` explícitas.
6. Completar solo tras superar ambas condiciones de integración, o registrar por qué no debe cambiar ninguna página.
7. Colocar una ingesta multicomando o una revisión amplia en un changeset, validar el borrador y publicarlo atómicamente.
8. Usar `search`, `context`, `graph` y `lint` para mantener la coherencia de la Wiki.

Consulta [docs/agent-workflow.md](../../docs/agent-workflow.md) para ver el contrato completo. Ejecuta `lwc --help` o `lwc <command> --help` para conocer precondiciones, transiciones, efectos y siguientes acciones.

## Cambios atómicos con varios comandos

Un comando `source` o `page` individual ya es transaccional. Usa un changeset cuando una actualización lógica necesite varios comandos y no pueda exponer una Wiki parcial:

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

Las lecturas del borrador ven las escrituras preparadas; SQLite y Markdown activos no cambian. La base del borrador comienza como una pequeña superposición dispersa: no copia ni crea checkpoints de la Wiki activa. `changeset show` informa de operaciones, revisiones y preparación sin ejecutar lint. El commit valida y aplica solo las entidades afectadas, por lo que sobreviven escrituras no relacionadas; un conflicto de revisión sobre la misma entidad falla sin sobrescribir ningún lado. Rechaza borradores vacíos y errores de lint; no hay force ni mezcla automática. Usa `--allow-lint-issues --reason "reviewed pre-existing debt"` solo para deuda auditada que el changeset no introdujo. Después del commit, repite las mismas comprobaciones fijas contra el estado activo. El commit congela el borrador revisado antes de publicarlo; `changeset_frozen` bloquea escrituras posteriores. Reintenta el mismo commit para recuperarte o descártalo tras un conflicto; no añadas trabajo a un borrador congelado.

```bash
lwc --scope project changeset discard architecture-refresh
lwc --scope project changeset rollback <CHANGESET_ID>
```

Discard solo toca un borrador no confirmado. Commit escribe un parche inverso con checksum que contiene únicamente las entidades afectadas y devuelve el ID exacto de rollback; rollback restaura solo esas entidades y se niega si alguna cambió después. Los changesets project y global están separados, `--scope all` no es válido, y `init`, `maintenance`, `checkpoint` y changesets anidados rechazan `--changeset`. Los borradores no crean una segunda proyección Markdown. Si un error estructurado indica `committed=true` pero quedan tareas de cleanup o materialización, no repitas los cambios: ejecuta la recuperación indicada.

El commit con superposición dispersa tiene parches exactos para Source add/ingest, Page put/remove, schema, purpose y búsquedas registradas. Los cambios de peso y relaciones semánticas explícitas fallan antes del checkpoint, del bloqueo de escritura o de modificar la Wiki con `changeset_sparse_unsupported`; aplícalos como transacciones directas sobre una entidad hasta disponer de sus parches inversos.

## Ámbitos

`lwc` admite tres ámbitos:

| Ámbito | Almacén | Uso |
| --- | --- | --- |
| `project` | `.lwc/wiki.db` del ancestro más cercano | Predeterminado, conocimiento del proyecto |
| `global` | `~/.lwc/wiki.db` | Conocimiento reutilizable entre proyectos |
| `all` | Almacenes project y global | Solo `search` y `context` combinados |

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
```

Las escrituras son explícitas. `all` no crea citas ni enlaces entre almacenes; `search --record` solo añade la operación de consulta a cada almacén seleccionado.

## Búsqueda y CJK

La búsqueda es léxica y determinista.

- Los términos son texto sencillo, no sintaxis FTS sin procesar.
- `--type auto` prioriza páginas compiladas, oculta sus fuentes emparejadas y usa las fuentes como respaldo.
- Usa `--type page`, `--type source` o `--type all` para elegir capa; repite `--kind` para limitar tipos.
- Los términos CJK de varios caracteres usan bigramas adyacentes; se conservan unigramas no vacíos para búsquedas de un carácter.
- El texto latino se tokeniza como términos alfanuméricos en minúsculas.
- Título, nombre de archivo, ruta/slug, resumen y cuerpo se puntúan por separado; las coincidencias de título y ruta reciben incrementos de puntuación limitados.
- README, índices, vistas generales y documentos centrales de navegación se devalúan según la consulta en favor de documentos específicos; pedir expresamente el README desactiva esa penalización.
- Los candidatos pueden recibir incrementos limitados por enlace directo o fuente compartida. Los vecinos comunes por sí solos no cambian el orden y los documentos de navegación demasiado generales reciben una penalización limitada.
- `--explain` devuelve la aritmética exacta de señales léxicas, genéricas, de grafo, peso manual y feedback. No registra la consulta; solo `--record` lo hace.
- Los coeficientes fijos y la regla «una puntuación menor indica mayor relevancia» permiten comparar los resultados project/global con `--scope all`.

No se utiliza un diccionario de segmentación. Así se mantiene un comportamiento estable con nombres de productos, nombres en clave, términos mixtos y vocabulario nuevo.

### Pesos y feedback explícitos

Usa un peso documental para un criterio duradero e independiente de la consulta. Usa feedback para la huella exacta de una consulta con tokens ordenados:

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

Los valores son `-2`, `-1`, `1` y `2`; usa `clear` para cero. Ambos mecanismos solo reordenan candidatos léxicos y no pueden hacer aparecer un documento que no coincida. Una fila `user-provided` prevalece sobre `agent-observed`, aunque ambas siguen auditables. El feedback guarda una huella SHA-256, no la consulta, y no se transfiere a paráfrasis con tokens distintos. Los motivos y operaciones son duraderos: no copies una consulta sensible en `--reason`. Las mutaciones requieren `project` o `global`; `--scope all` se rechaza.

## Visor de solo lectura y CodeGraph

`lwc view` inicia en primer plano un inspector de proyecto limitado al loopback y abre el navegador. Sirve una única aplicación TS + Lit embebida, sin CDN ni runtime de Node durante el uso, y solo expone APIs GET/HEAD. Las páginas, fuentes, Markdown, el grafo de conocimiento y el grafo de código opcional se leen del proyecto actual sin migrar, actualizar ni construir grafos:

```bash
lwc view
lwc view --port 4173 --no-open
```

El visor comienza en inglés. Usa el control `中文` / `EN` para cambiar de idioma; el navegador recuerda la elección mientras el contenido de la Wiki permanece en el idioma en que se escribió. Ambos grafos usan una vista 3D de relaciones inspirada en Obsidian, con nodos pequeños, etiquetas persistentes, enlaces finos, rotación y zoom.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="Inteligencia de código de LWC CodeGraph" width="100%"></p>

El indexado de código solo existe en project y permanece desactivado hasta que se inicializa expresamente. El fork fijado de CodeGraph se descarga una vez desde GitHub Releases, se verifica con SHA-256 y se guarda en `~/.lwc/runtime/codegraph/<PIN>/<TARGET>/`; cada proyecto conserva únicamente su índice en `.lwc/codegraph`. La telemetría siempre está desactivada y no se usa estado `.codegraph`.

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

La versión fijada del runtime reconoce estos lenguajes y formatos relacionados con código: TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust, Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro, Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig, XML, `.properties`, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang y Terraform. YAML, Twig y `.properties` se rastrean a nivel de archivo; los analizadores de frameworks aún pueden añadir relaciones. XML se reconoce para extraer mappers de MyBatis.

`lwc cg` reenvía todas las consultas de CodeGraph. Los comandos globales de ciclo de vida (`install`, `uninstall`, `upgrade`, `telemetry`, `daemon`, `daemons`) están bloqueados. El puente exacto `lwc cg serve --mcp` se mantiene para compatibilidad manual antigua; las integraciones nuevas usan `lwc serve --mcp`, que reúne la exploración acotada de Wiki y CodeGraph tras una sola herramienta de solo lectura. LWC controla el runtime y aplica el límite del proyecto. Las escrituras iniciales, incrementales, completas, de actualización, eliminación, resolución y recuperación confirman por completo cada archivo al que pertenecen antes de pasar al siguiente; el grafo actual sigue disponible y las revisiones históricas nunca se actualizan.

## Mantenimiento y proyección

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

Notas:

- Los comandos de mantenimiento devuelven de inmediato un `work` duradero. Consulta `work status` o espera con `work watch` y revisa `work.result` tras el éxito. La migración de schema v10 a v11 usa el mismo mecanismo; los comandos normales no la ejecutan inline.
- `lint` es de solo lectura de forma predeterminada. Añade `--record` únicamente si la revisión debe formar parte del historial duradero.
- `maintenance reindex` reconstruye artefactos de búsqueda derivados desde SQLite.
- `maintenance materialize` reconstruye el árbol Markdown proyectado desde SQLite.
- `maintenance compact` solo intenta un checkpoint WAL truncate; no oculta una optimización FTS completa. Ejecútalo con la Wiki inactiva y revisa `busy` y `after_bytes`. Un lector ocupado vuelve pronto sin cambiar contenido canónico.
- Las consultas son privadas de forma predeterminada; añade `--record` solo si deseas guardar su texto en el registro duradero.

`lwc checkpoint create <NAME>` usa la API de copia de seguridad en línea de SQLite. Restaura con `lwc checkpoint restore <NAME>`; antes LWC crea un checkpoint de seguridad `pre-restore-*` y después reconstruye la proyección. Usa `source remove <ID>` y `page remove <SLUG>` para eliminar con protección: se rechazan fuentes citadas y páginas con enlaces entrantes. Eliminar la fuente actual de una ruta rastreada detiene el seguimiento sin exponer silenciosamente una revisión antigua como actual.

Para ingestas de varias fuentes o sustituciones amplias, prefiere un changeset a un checkpoint manual: un commit correcto escribe un parche inverso disperso, publica solo las entidades afectadas en una transacción y materializa incrementalmente el Markdown cambiado. Tras publicar intenta truncar el WAL; `wal_checkpointed=false` significa que un lector activo lo impidió, no que fallara el commit canónico.

Para una copia externa, detén los comandos `lwc` activos y copia todo `.lwc/`. No copies solo `wiki.db` mientras un escritor pueda estar usando sus archivos WAL.

## Suite de benchmarks

El benchmark opcional importa un corpus UTF-8 local en una Wiki temporal e informa del tiempo de importación, P50/P95 de búsqueda, Recall@5/10, MRR y almacenamiento antes/después de compactar. El conjunto de referencia es un JSONL de consultas y rutas relativas esperadas:

```bash
cargo build --release
LWC_BENCH_CORPUS=/path/to/sanitized-corpus \
LWC_BENCH_QUERY_SET=/path/to/query-set.jsonl \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
cargo test --test search_benchmark -- --ignored --nocapture
```

`cargo test --all-targets` cubre búsquedas page-first, filtros type/kind, ventanas UTF-8, condiciones de ingesta, precisión del grafo, migraciones, lint y compactación WAL. Consulta [benchmarks/README.md](../../benchmarks/README.md) para el contrato de carga y las reglas de comparación justa.

## Límites y objetivos excluidos

Restricciones actuales:

- base de conocimiento para una sola máquina y un solo usuario;
- flujo de texto UTF-8;
- límite de 64 MiB por schema, purpose, source o cuerpo de página;
- búsqueda léxica, no recuperación vectorial semántica.

Objetivos excluidos deliberadamente:

- sin llamadas LLM incorporadas;
- sin base de datos vectorial;
- sin daemon ni servicio en segundo plano;
- sin interfaz web ni de escritorio;
- sin contrato para editar directamente la base de datos.

Si la proyección Markdown se desvía, reconstrúyela. Si el esquema SQLite está mal, corrígelo mediante la CLI y migraciones, no a mano.

## Contribuir

Se aceptan issues y pull requests, especialmente sobre:

- ergonomía del flujo de agentes;
- proyección determinista;
- contratos duraderos de citas y mantenimiento de páginas;
- calidad de búsqueda en corpus técnicos multilingües.

Lee [CONTRIBUTING.md](../../CONTRIBUTING.md) antes de abrir un pull request. Informa de problemas de seguridad según [SECURITY.md](../../SECURITY.md).

## Licencia

Publicado bajo la [Apache License 2.0](../../LICENSE).
