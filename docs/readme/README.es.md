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

LWC separa el conocimiento duradero en capas claramente delimitadas:

| Capa | Finalidad |
| --- | --- |
| Fuentes originales | Instantáneas inmutables de evidencia seleccionada |
| Wiki | Páginas, citas, enlaces y procedencia mantenidos por el agente |
| Esquema y propósito | Reglas del proyecto que guían el mantenimiento futuro |

SQLite es la fuente de verdad. Markdown, los índices de texto completo y los
grafos opcionales son proyecciones reconstruibles. Las operaciones devuelven
JSON estructurado para facilitar la auditoría y la recuperación.

[Ver la arquitectura →](https://github.com/JanYork/llm-wiki-cli/wiki/Architecture-Overview)

## Recuperación jerárquica y grafo de conocimiento

LWC indexa las fuentes y las páginas de la Wiki a nivel de documento, pasaje y
oración. El agente puede empezar con un contexto pequeño y relevante y ampliar
solo el fragmento exacto que necesita.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="Grafo de memoria de LWC" width="100%"></p>

El grafo documental opcional conecta páginas, fuentes, citas, enlaces y
relaciones semánticas explícitas. SQLite sigue siendo la autoridad; Grafeo o
SurrealDB aportan una capa de recorrido reconstruible. Cada relación conserva
su motivo, procedencia, confianza y evidencia.

### Conversión de documentos y lectura de Office

Los adaptadores opcionales Anydoc o MarkItDown convierten archivos locales
compatibles en Markdown revisable antes de incorporarlos. OfficeCLI ofrece una
vía separada, de solo lectura y sujeta a consentimiento para Word, Excel y
PowerPoint. Ninguna capacidad se instala ni activa en silencio, y los archivos
de Office originales no se modifican.

[Recuperación e indexación →](https://github.com/JanYork/llm-wiki-cli/wiki/Retrieval-and-Indexing) ·
[Grafo documental →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Knowledge-Graph) ·
[Conversión de documentos →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Conversion)

## Instalación

Para la mayoría de los usuarios basta con un comando:

    npm install --global @i-xor/lwc

También se admiten Homebrew, crates.io, las versiones de GitHub verificadas
mediante suma de comprobación y las compilaciones locales con Cargo.

[Instalación y actualizaciones →](https://github.com/JanYork/llm-wiki-cli/wiki/Installation-and-Upgrades)

## Skill complementario para agentes

El [Skill using-lwc](../../skills/using-lwc) incluido convierte LWC en una capa
de memoria proactiva. Recupera contexto acotado, separa el conocimiento de
proyecto del global, integra fuentes, mantiene las citas y solo conserva
conocimiento verificado que merece reutilizarse.

Se instala desde [skills.sh](https://skills.sh/JanYork/llm-wiki-cli):

    npx skills add JanYork/llm-wiki-cli --skill using-lwc -g

La invocación canónica es <code>$using-lwc</code>. El Skill es independiente
del agente e incluye guías específicas para memoria, grafos documentales, Word
Graph, CodeGraph, etiquetas fuertes, conversión, configuración, recuperación y
mantenimiento.

### Configuración nativa de agentes

LWC detecta los agentes compatibles y configura sus superficies MCP, Skill,
Hook e Instructions mediante adaptadores AgentTarget idempotentes:

    lwc agent install --yes

El MCP unificado y de solo lectura ofrece memoria Wiki acotada y contexto de
código opcional sin ampliar el espacio de trabajo. Es compatible con Claude
Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity y pi.

[Integración con AgentTarget →](https://github.com/JanYork/llm-wiki-cli/wiki/AgentTarget-Installation-and-Integration)

## Inicio rápido

Normalmente, la persona describe el objetivo y revisa el resultado; el agente
maneja la CLI. El recorrido completo está en la
[guía de inicio rápido](https://github.com/JanYork/llm-wiki-cli/wiki/Quick-Start).

### 1. Inicializar una Wiki de proyecto

El agente crea una Wiki local del proyecto y define su propósito y reglas de
mantenimiento. El estado se excluye localmente de Git salvo que se decida
versionarlo de forma explícita.

### 2. Añadir material fuente

Los archivos seleccionados se convierten en instantáneas inmutables y sin
duplicados. LWC rastrea sus rutas y puede indicar si el archivo actual no ha
cambiado, se ha modificado, falta o ha sido reemplazado.

### 3. Analizar e integrar una fuente

El agente lee la fuente completa dentro de límites explícitos, escribe un
resumen con citas, actualiza el conocimiento compartido y solo entonces da por
terminada la incorporación.

### 4. Consultar la Wiki acumulada

La búsqueda prioriza las páginas mantenidas sin perder el vínculo con la
evidencia. El agente abre el texto original exacto cuando una afirmación exige
verificación.

## Flujo de trabajo del agente

El ciclo normal consiste en recuperar conocimiento relevante, comprobar fuentes
o código actuales cuando importa la vigencia, realizar la mínima actualización
verificada y validar la recuperación, los enlaces y los grafos aplicables. Las
revisiones amplias se publican de forma atómica mediante un changeset.

[Flujo de trabajo completo →](../../docs/agent-workflow.md)

## Cambios atómicos con varios comandos

Un changeset mantiene oculta una actualización de varios pasos hasta que haya
sido revisada y validada. El commit publica en una sola transacción únicamente
las entidades afectadas, conserva el trabajo ajeno y falla de forma segura ante
un conflicto de revisión de la misma entidad.

Para las operaciones compatibles se conserva un parche inverso exacto, lo que
permite un rollback protegido sin reemplazar toda la Wiki.

[Guía de changesets →](https://github.com/JanYork/llm-wiki-cli/wiki/Changesets)

## Ámbitos

| Ámbito | Uso |
| --- | --- |
| project | Conocimiento perteneciente a la Wiki del proyecto más cercano |
| global | Conocimiento reutilizable entre proyectos |
| all | Recuperación combinada de solo lectura y Sync coordinado |

Las escrituras siempre apuntan a un único almacén explícito; LWC no crea citas
ni enlaces implícitos entre proyectos.

[Ámbitos y detección de proyectos →](https://github.com/JanYork/llm-wiki-cli/wiki/Scopes-and-Project-Discovery)

## Búsqueda y CJK

La búsqueda es léxica, determinista y prioriza las páginas mantenidas. Puntúa
por separado título, ruta, resumen, cuerpo, procedencia y evidencia del grafo;
admite filtros de página, fuente y tipo, y puede explicar el cálculo exacto.

Para CJK usa bigramas adyacentes y unigramas útiles; para texto latino,
términos alfanuméricos en minúsculas. Al no depender de diccionarios, mantiene
un comportamiento estable con nombres de productos, símbolos de código, texto
multilingüe y vocabulario emergente.

### Pesos y feedback explícitos

Los pesos auditables expresan la importancia duradera de un documento. El
feedback de una consulta solo reordena candidatos coincidentes y guarda una
huella, no la consulta original. Ninguno puede hacer aparecer contenido ajeno.

[Búsqueda y contexto →](https://github.com/JanYork/llm-wiki-cli/wiki/Search-and-Context)

## Visor de solo lectura y CodeGraph

El visor local presenta páginas, fuentes, Markdown, relaciones documentales y
estructura del código mediante una interfaz de bucle local limitada a GET/HEAD.
No migra, actualiza ni construye grafos.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="Inteligencia de código de LWC CodeGraph" width="100%"></p>

CodeGraph es exclusivo del proyecto y se inicializa de forma explícita. Permite
consultar símbolos, llamadores, llamados, dependencias, archivos e impacto,
mantiene la telemetría desactivada y actualiza el grafo de forma atómica por
archivo propietario.

El runtime reconoce TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust,
Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro,
Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig,
XML, .properties, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang y Terraform.

[Visor →](https://github.com/JanYork/llm-wiki-cli/wiki/Read-Only-Viewer) ·
[CodeGraph →](https://github.com/JanYork/llm-wiki-cli/wiki/Code-Graph)

## Mantenimiento y proyección

El lint, la reindexación, la materialización de Markdown, la compactación, los
checkpoints y la proyección de grafos son operaciones explícitas. El trabajo
largo es duradero, observable, reanudable y se aplica por unidades documentales
acotadas.

SQLite sigue siendo la autoridad. Los índices, Markdown y grafos pueden
reconstruirse sin reescribir la historia de las fuentes ni el conocimiento
actual de la Wiki.

[Mantenimiento y diagnóstico →](https://github.com/JanYork/llm-wiki-cli/wiki/Maintenance-and-Diagnostics)

## Suite de benchmarks

El benchmark opcional mide tiempo de importación, latencia de búsqueda,
Recall@5/10, MRR y almacenamiento sobre un corpus saneado aportado por el
usuario. Una comparación justa fija máquina, corpus, consultas y condiciones,
y compara medianas de varias ejecuciones.

[Metodología →](../../benchmarks/README.md)

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
