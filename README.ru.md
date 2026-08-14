<h1 align="center">LWC — проактивная память для ИИ-агентов</h1>

<p align="center"><strong>Управляется агентами · Сохраняется между сеансами · Опирается на источники</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/@i-xor/lwc"><img alt="npm: @i-xor/lwc" src="https://img.shields.io/badge/npm-%40i--xor%2Flwc-CB3837?logo=npm"></a>
  <a href="https://crates.io/crates/lwc"><img alt="crates.io: lwc" src="https://img.shields.io/crates/v/lwc.svg"></a>
  <img alt="Node.js 22 или новее" src="https://img.shields.io/badge/node-%3E%3D22-5FA04E?logo=nodedotjs">
  <img alt="Платформы: macOS, Linux и Windows" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-666666">
  <a href="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://skills.sh/janyork/llm-wiki-cli/using-lwc"><img alt="skills.sh: using-lwc" src="https://img.shields.io/badge/skills.sh-using--lwc-000000?logo=vercel"></a>
  <a href="LICENSE"><img alt="Лицензия: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a> · <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> · <a href="README.fr.md">Français</a> ·
  <a href="README.ru.md">Русский</a>
</p>

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-social-preview.png" alt="LWC — проактивная память для ИИ-агентов" width="100%"></p>

`lwc` — это управляемый агентами интерфейс командной строки для проактивной памяти ИИ-агентов. Он позволяет агентам самостоятельно находить, поддерживать и развивать знания, которые сохраняются между сеансами и остаются привязанными к своим источникам.

**Работает с Claude Code, Codex, Cursor, OpenCode, Gemini CLI, Kiro, Hermes, Antigravity и pi.**

LWC превращает отобранные документы в долговечную Wiki. Агент рассуждает и обобщает, а `lwc` хранит источники, страницы, цитаты, ссылки, индексы и историю. Знания накапливаются, а не собираются заново из сырых фрагментов при каждом запросе.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-overview-en.png" alt="Обзор LWC" width="820"></p>

## LWC — память агента, а не RAG

RAG и LWC помогают LLM работать с внешними документами, но сохраняют состояние в разных местах. Обычный RAG-запрос извлекает сырые фрагменты и формирует разовый ответ:

```text
query -> retrieve chunks -> generate answer
```

LWC сохраняет полезную работу между запросами:

```text
task -> recall maintained Wiki -> reason from sources and prior synthesis
     -> write durable improvements back
```

Поиск — одна из операций LWC, а не принцип устройства всей системы. Долговечный результат — это Wiki, основанная на источниках: её страницы, цитаты, ссылки, противоречия и история пересматриваются по мере изменения знаний. Поэтому LWC не требует эмбеддингов или векторной базы и не выбрасывает результат обобщения после ответа. LWC может дополнять RAG, но сам не является RAG, выполняемым во время каждого запроса.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-source-grounding-en.png" alt="Источники и трассируемость в LWC" width="820"></p>

### LWC управляет агент

`lwc` — машинный интерфейс для агентов, а не приложение для заметок. В обычном сценарии человек выбирает источники, задаёт цели и вопросы, затем проверяет ответы или проекцию Markdown. Агент запускает CLI, управляет областями, интегрирует источники, поддерживает цитаты и ссылки и решает, что стоит вспомнить или записать обратно.

Не управляйте повседневным процессом `lwc` вручную, если только не разрабатываете или не отлаживаете инструмент. Попросите агента активировать канонический Skill `using-lwc`, обычно командой `$using-lwc`.

## Рекомендуется: поручите агенту настройку LWC

Вставьте следующий запрос в используемого агента. Он установит глобальную CLI, передаст настройку поддерживаемых хостов идемпотентному установщику AgentTarget и воспользуется штатной настройкой самого агента только для незарегистрированного хоста.

<details>
<summary><strong>Скопировать полный запрос для настройки</strong></summary>

```text
Полностью настрой LWC для этого пользователя. Выполни и проверь работу, а не
просто перечисли команды, которые мне нужно запустить.

Источники истины:
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

Требования:
1. Прочитай этот README, `SECURITY.md` и `skills/using-lwc/SKILL.md`. Если `lwc`
   нельзя вызвать глобально, установи официальную версию с проверенной контрольной
   суммой. Не добавляй к обычным командам закрытый путь к бинарному файлу или
   `LWC_PROJECT_ROOT`.
2. Выполни `lwc --version`. Если глобальной памяти нет, один раз инициализируй её
   через `lwc --scope global init`, затем выполни `lwc agent install --yes`. Эта
   команда обнаруживает установленных поддерживаемых агентов и безопасно ставит их
   MCP, Skill, Hook и Instructions в официальные каталоги. Не воспроизводи эту
   логику вручную и не устанавливай нативный пакет для того же агента одновременно.
3. Проверь `lwc agent status --target all --location global`. Перезапусти затронутые
   агенты и пройди штатную проверку доверия к Hooks, если она требуется. Не
   инициализируй проектную Wiki или графы без явного согласия для проекта.
4. Если текущая среда выполнения не зарегистрирована как AgentTarget LWC, следуй её
   официальным пользовательским соглашениям: установи канонический Skill
   `using-lwc`, добавочный блок инструкций, `lwc serve --mcp` и ограниченный Hook
   сеанса — только там, где эти поверхности официально поддерживаются. Сохрани
   существующую конфигурацию и идемпотентность; неподдерживаемые поверхности
   перечисли явно, не придумывая пути или ключи.

В конце укажи версию LWC, обнаруженные и настроенные Targets, результаты status,
изменённые файлы, неподдерживаемые поверхности и оставшиеся действия по перезапуску
или доверию.
```

</details>

## Происхождение и благодарности

`lwc` реализует предложенный Андреем Карпати подход [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f): LLM постепенно строит и поддерживает постоянную связанную Wiki, а не восстанавливает знания из сырых документов при каждом запросе. Архитектура CLI и некоторые детали также вдохновлены проектом [`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki).

Проект адаптирует эти идеи в ориентированную на агентов CLI на Rust с SQLite.

## Основная архитектура

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-architecture-en.png" alt="Архитектура LWC" width="100%"></p>

Модель постоянных знаний состоит из трёх логических слоёв:

| Слой | Содержимое | Контракт |
| --- | --- | --- |
| Сырые источники | Неизменяемые снимки отобранных материалов | Добавлять через `source`; не переписывать исходную истину. |
| Wiki | Страницы, цитаты, ссылки и происхождение, поддерживаемые агентом | Обновлять через `page`; ссылаться на источники и классифицировать долговечные знания без источника. |
| Схема и назначение | Правила обслуживания и цель проекта | Направляют все последующие операции ingest и правки. |

Каноническое состояние хранится в SQLite. Дерево Markdown — восстанавливаемая проекция для людей и инструментов вроде Obsidian. Агенты меняют знания через `lwc`, не редактируя напрямую `.lwc/wiki.db` или спроецированный Markdown. Успешные команды выводят JSON в stdout, ошибки — структурированный JSON в stderr.

Команды чтения оставляют хранилища текущего формата неизменными. Когда новая CLI впервые открывает старое доступное для записи хранилище, она один раз транзакционно мигрирует схему до продолжения чтения.

## Иерархическое извлечение и граф знаний

Каждый текущий Source и страница Wiki детерминированно индексируются по отрывкам и предложениям. SQLite остаётся авторитетным состоянием; FTS по spans и необязательный внешний граф документов — восстанавливаемые индексы. Обычный поиск возвращает только документы, если другая гранулярность не запрошена.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="Граф памяти LWC" width="100%"></p>

```bash
lwc search "projection consistency" --granularity sentence --type page
lwc search "projection consistency" --granularity passage
lwc search "projection consistency" --granularity all --group-by document
lwc span get <SPAN_ID>
lwc span expand <SPAN_ID> --before 1 --after 1 --children 20
```

Локаторы spans содержат отпечаток документа и версию сегментации. Локатор заменённого тела завершается с `stale_span` и возвращает прошлые и текущие метаданные; LWC никогда молча не переназначает его похожему тексту.

Для исследования без ключевых слов используйте ограниченный типизированный API графа:

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

Автоматические рёбра ограничены структурными или доказуемыми фактами. Семантические отношения должны быть явными и проверяемыми:

```bash
lwc graph relation set page:implementation DEPENDS_ON page:policy \
  --provenance source-grounded --source 12 \
  --reason "Source 12 states the required policy" --confidence 0.95
lwc graph relation list --from page:implementation
lwc graph relation retract page:implementation DEPENDS_ON page:policy \
  --reason "The dependency was superseded"
```

Причины отношений сохраняются надолго: не помещайте туда учётные данные, секреты или необработанную цепочку рассуждений.

Документы SQLite остаются авторитетными. Графовое хранилище по умолчанию отключено; включите ровно один внешний движок, когда нужен обход. Конфигурация складывается из встроенного, глобального и проектного уровней:

```bash
lwc config show
lwc config set --graph grafeo
lwc config set --graph surrealdb
lwc config set --graph disabled
lwc config unset --graph
```

Преобразование Markdown — отдельная необязательная операция. `lwc init` показывает те же машиночитаемые указания, но не устанавливает и не включает конвертер. Установите один адаптер, явно выберите его, преобразуйте вход в новый локальный Markdown, проверьте результат и только затем добавьте источник:

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

Конфигурация принимает `--trans-timeout 1..900` и несколько `--trans-arg=<value>`. LWC напрямую запускает выбранный бинарный файл, не переключается на другой адаптер, принимает только локальные файлы, ограничивает ввод и вывод 64 МиБ и не перезаписывает существующий результат. Учётные данные храните в окружении адаптера, а не в конфигурации LWC. Поддерживаемые форматы и параметры описаны в документации [Anydoc](https://github.com/firecrawl/anydoc) и [MarkItDown](https://github.com/microsoft/markitdown).

Grafeo и встроенный SurrealDB используют одноразовые вспомогательные хранилища в `.lwc/`. Каждый Work `graph-project` полностью фиксирует текущий Source/Page, принадлежащие ему ссылки, цитаты и явные отношения до перехода к следующему документу. Обновления и удаления ставят в очередь только затронутые документы; `rebuild` и `resume` используют те же единицы. Исторические ревизии неизменяемы и никогда не токенизируются или проецируются заново. Следите через `work list`, `work status` или `work watch`, а после прерывания используйте `work resume`. `graph status` сообщает движок и число документов; `graph verify` сверяет текущие ключи с SQLite.

## Установка

Большинству пользователей подходит запрос настройки выше. Ручные команды предназначены для обслуживания, отладки или сред без возможности установить Skill.

Homebrew (готовые Bottles для macOS Apple silicon и Linux x86_64):

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

Установщик поддерживает macOS x86_64/aarch64, Linux glibc и Windows Git Bash, проверяет контрольную сумму и устанавливает или обновляет `lwc`. По умолчанию используется `~/.local/bin`; существующая копия в `~/.local/bin` или `~/.cargo/bin` обновляется. Другой каталог:

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | LWC_INSTALL_DIR="$HOME/bin" sh
```

Сборка из GitHub через Cargo:

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

Установка из локальной копии репозитория:

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## Сопутствующий Skill для агентов

Репозиторий включает [`skills/using-lwc`](skills/using-lwc) — Agent Skill, который использует `lwc` как проактивный слой памяти в содержательных сеансах. Установка через [skills.sh](https://skills.sh/JanYork/llm-wiki-cli):

```bash
npx skills add JanYork/llm-wiki-cli --skill using-lwc -g
```

Можно также скопировать его из локальной копии репозитория в пользовательский каталог Skills текущей среды выполнения. Для Codex:

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/using-lwc "$HOME/.agents/skills/"
```

Канонический вызов — `$using-lwc`.

После активации Skill:

- находит совместимую CLI или устанавливает официальную версию с проверенной контрольной суммой;
- один раз инициализирует глобальную память в `~/.lwc/`;
- извлекает ограниченный глобальный и проектный контекст до повторного исследования;
- инициализирует текущий проект при явном вызове, иначе сначала спрашивает;
- запрещает проектную запись за пределами разрешённого корня workspace;
- отделяет факты проекта от переиспользуемых глобальных знаний;
- интегрирует источники и записывает долговечные ответы обратно в Wiki.

`SKILL.md` — короткий маршрутизатор, а не монолитное руководство. Он ведёт к отдельным документам о базовой и активной памяти, правилах активации, физическом графе, ограниченном Word Graph, CodeGraph, strong tags, конвертации, подключении агентов и восстановлении/обслуживании. В каждом указаны условия применения и пропуска, минимальный процесс, граница согласия и доказательства завершения.

Обычно Skill определяет проект по текущему каталогу и напрямую вызывает глобальный `lwc`. `LWC_PROJECT_ROOT` — явная граница намеренно выбранного проекта, а не префикс для повседневных команд в текущем проекте.

`LWC_AUTO_INSTALL=0` отключает автоматическую установку. Она запускает проверенный установщик из Skill, доверяет этому репозиторию и контуру публикации GitHub Releases и сверяет архив с `SHA256SUMS`; контрольная сумма обеспечивает целостность, но не является подписью издателя. Бинарные файлы охватывают macOS x86_64/aarch64, Linux glibc и Windows через Git Bash. `SKILL.md` следует структуре Agent Skills, а `agents/openai.yaml` содержит метаданные OpenAI/Codex. CLI не зависит от среды выполнения: любой агент, способный запустить её и загрузить или адаптировать инструкции, может использовать LWC. Команды Skill, глобальные инструкции и Hooks зависят от среды выполнения, поэтому запрос настройки определяет текущий хост.

### Нативная настройка агентов

LWC обнаруживает поддерживаемых агентов и устанавливает единый MCP LWC только для чтения. Все 12 зарегистрированных AgentTargets — полные адаптеры: они устанавливают доступные официальные файловые точки интеграции MCP, Skill, Hook и Instructions для каждого хоста и области, а точки под управлением интерфейса, в предварительной версии или без поддержки помечают явно.

```bash
lwc agent install --yes
lwc agent status --target all --location global
lwc agent install --print-config codex
lwc agent refresh --target codex,claude
lwc agent uninstall --target codex,claude --yes
```

`--yes` выбирает обнаруженных агентов, глобальную область и Hooks жизненного цикла/prompt по умолчанию. `--no-prompt-hook` отключает Hook Claude для каждого prompt. Устанавливается запись `lwc -> serve --mcp`; единственный инструмент `lwc_explore` по умолчанию читает ограниченную память Wiki и принимает режимы `code` и `all`. `projectPath` обязан находиться внутри workspace, где MCP-хост запустил LWC. Инструмент никогда не скачивает и не инициализирует CodeGraph. Повторные install и refresh идемпотентны побайтно; uninstall восстанавливает только состояние LWC и сохраняет индексы. Необязательные пакеты Codex, Claude Code и Pi лежат в `integrations/`. Пакет не даёт и не обходит нативное доверие. Не совмещайте прямой установщик и нативный пакет для одного агента. Каждый пакет содержит полный `using-lwc` и не зависит от стороннего менеджера Skills или среды сопровождающего.

Pi публикует MCP LWC через официальный мост расширений, поскольку не имеет встроенного MCP. Другие Targets регистрируют только `lwc serve --mcp`; CodeGraph остаётся внутренним уровнем контекста и не становится вторым MCP. Настройки доверия и разрешений, принадлежащие UI, остаются за пользователем. Preview-поверхности помечаются, а частичная проектная поддержка устанавливает доступные части, не ослабляя и не отклоняя весь Target. Глобальные пути Kiro учитывают `KIRO_HOME`.

Интерфейс Target, порядок реестра, правила обнаружения и пути MCP следуют адаптеру установщика CodeGraph под MIT. LWC добавляет единый MCP, отчёт по поверхностям, Skills и Hooks, владение общими файлами и точный rollback. См. [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

Вывод `lwc init` и Hooks начала/сжатия показывают ограниченные факты `LWC_READINESS` о Wiki, физическом графе, среде выполнения и индексе CodeGraph, а также команды интеграции. Готовность графа различает настроенное согласие и ожидающую или неудачную проекцию. Обнаружение только читает и не включает графы. Когда оба требуют разрешения, переносимая основа — простой текст:

```text
1. Enable physical document graph and CodeGraph (recommended)
2. Enable physical document graph only
3. Enable CodeGraph only
4. Later
```

После явного выбора `1` агент при необходимости инициализирует Wiki, включает Grafeo, ждёт и проверяет Work проекции, инициализирует CodeGraph и независимо проверяет оба результата. `Later` ничего не меняет и не блокирует задачу. Плагины могут показать те же ID в своём UI; checkbox не обязателен.

Strong tags явно и ограниченно загружают целиком несколько основных правил или runbooks:

```bash
lwc tag set "operations" incident-response --priority 100 --reason "primary runbook"
lwc load tag "operations" --limit 3
lwc tag autoload "operations" --enable --priority 100 --limit 3 \
  --max-chars 50000 --reason "required at session boundaries"
```

Это не поиск, выведенный из токенов: лимиты страниц и символов применяются до помещения полных страниц в контекст.

## Быстрый старт

Раздел описывает протокол CLI, который выполняет агент. В обычной работе человеку не нужно запускать эти команды.

### 1. Инициализация проектной Wiki

```bash
cd your-project
lwc init
printf '# Schema\nEvery page declares provenance; source-grounded claims cite sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

Инициализация при необходимости добавляет `.lwc/` в локальный Git `info/exclude`, не меняя `.gitignore`. Используйте `lwc init --no-git-exclude`, только если Wiki намеренно версионируется.

### 2. Добавление материалов

```bash
lwc source add-dir docs/
```

Файлы без заголовка используют origin источника как стабильное читаемое имя. Одинаковые байты устраняются по SHA-256. Источники за активным корнем требуют `--allow-external-source`. Маркеры учётных данных с высокой уверенностью отклоняются, пока проверенный источник явно не подтверждён через `--acknowledge-sensitive-source`.

Каждое добавление фиксирует наблюдаемый путь и текущий неизменяемый снимок. Перед использованием файлового доказательства проверяйте только относящиеся источники:

```bash
lwc source status 7 12
```

Команда потоково вычисляет SHA-256 и отдельно сообщает lineage (`current` или `superseded`) и состояние файла (`current`, `modified`, `missing`, `unreadable`, `oversized`, `unstable`). Она только читает. `source status --all` стоит пропорционально всем отслеживаемым байтам, поэтому нужен лишь при обслуживании. Изменённый путь сначала проверьте:

```bash
lwc source diff 7
lwc source refs 7 --limit 1000
```

`source diff` сравнивает неизменяемый источник с текущим файлом или другим снимком через `--to-source`. Diff ограничен 8 МиБ и 200 000 строк на сторону, 20 000 символов Unicode по умолчанию и 100 000 с `--max-chars`. При нескольких путях укажите `--path`. Усечённый diff — только предварительный просмотр. `source refs` перечисляет прямые ссылки-кандидаты, но не доказывает семантическое влияние. Повторяйте `source add` лишь после проверки значимой ревизии. A -> B -> A сохраняет три наблюдения, даже если A повторно использует исходный source ID. Внешний путь снова требует `--allow-external-source`, а отмеченный текст — `--acknowledge-sensitive-source` после проверки.

Источники из старых хранилищ остаются явно неотслеживаемыми: LWC не угадывает старые пути. Повторно добавьте файл один раз. Если файл или текущая ревизия пути меняется во время проверки, возвращается `source_status_unstable`; повторите операцию.

Для атомарного импорта пути JSON manifest разрешаются относительно его каталога:

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

### 3. Анализ и интеграция источника

```bash
lwc ingest next --context-limit 50 --source-max-chars 100000
lwc ingest analyze 1 --file analysis.md
```

Используйте `lwc ingest claim 7`, если manifest или scheduler уже выбрал ожидающий source ID.

Если `source_window.has_more` равен true, продолжите с `source_window.next_offset_chars`:

```bash
lwc source show 1 --offset-chars 100000 --max-chars 100000
```

До завершения создайте цитируемую source-summary страницу и интегрируйте вклад хотя бы в одну не-source страницу:

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

Нужны оба слоя: source-страница помогает навигации и происхождению, общая страница накапливает знания. Если источник действительно не меняет общую страницу, завершите его с конкретным проверяемым объяснением:

```bash
lwc ingest complete 1 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

Цитаты автоматически дают происхождение `source-grounded`. Для знаний от пользователя, наблюдения агента или гипотезы повторяйте `--provenance`, не выдумывая источник:

```bash
lwc page put architecture-decision \
  --title "Architecture decision" \
  --kind query \
  --summary "Accepted constraint and remaining uncertainty" \
  --file decision.md \
  --provenance user-provided \
  --provenance hypothesis
```

`page put` заменяет полный набор цитат и явных данных о происхождении. Сначала прочитайте страницу и повторите каждый действующий `--source` и `--provenance`. Не передавайте `source-grounded`: он выводится из цитат. Данные о происхождении видны в page, context, search, refs и проекции, но не влияют на ранжирование.

### 4. Поиск в накопленной Wiki

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc search "question keywords" --limit 20 --explain
lwc search "concept only" --type page --kind concept
lwc search "exact evidence" --type source
lwc page show source-1
```

## Процесс агента

1. Собрать неизменяемые источники.
2. Получить задачу через ограниченный `lwc ingest next` или `ingest claim <ID>` для выбранного источника.
3. Прочитать все окна, schema, purpose и ограниченный context.
4. Анализировать до генерации страниц.
5. Создать или обновить summary и общие страницы с явными цитатами `--source`.
6. Завершить после обеих проверок или записать причину отсутствия изменений.
7. Поместить многошаговый ingest или широкую правку в changeset, проверить черновик и атомарно опубликовать.
8. Поддерживать связность через `search`, `context`, `graph` и `lint`.

Полный контракт — в [docs/agent-workflow.md](docs/agent-workflow.md). `lwc --help` и `lwc <command> --help` показывают предусловия, переходы, эффекты и следующие действия.

## Атомарные многошаговые изменения

Одиночная команда `source` или `page` транзакционна. Используйте changeset, когда логическое изменение требует нескольких команд без публикации промежуточной Wiki:

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

При чтении черновика видны подготовленные изменения, а рабочие SQLite и Markdown остаются неизменными. База черновика — небольшой разреженный слой изменений без копирования или checkpoint рабочей Wiki. `changeset show` сообщает операции, ревизии и готовность без lint. Commit проверяет только затронутые сущности, сохраняя посторонние записи; конфликт одной сущности завершается без перезаписи сторон. Пустой черновик и lint-ошибки отклоняются; принудительной фиксации и автоматического слияния нет. `--allow-lint-issues --reason "reviewed pre-existing debt"` допустим только для проверенного старого долга. После commit повторите те же проверки рабочего состояния. Commit замораживает черновик; `changeset_frozen` блокирует новые записи. Для восстановления повторите тот же commit или удалите черновик после конфликта, не добавляя новую работу.

```bash
lwc --scope project changeset discard architecture-refresh
lwc --scope project changeset rollback <CHANGESET_ID>
```

Discard затрагивает лишь неподтверждённый черновик. Commit пишет inverse patch с checksum для изменённых сущностей и возвращает точный ID; rollback восстанавливает только их и отказывает, если сущность менялась. Project/global changesets раздельны, `--scope all` недопустим, `init`, `maintenance`, `checkpoint` и вложенные changesets отклоняют `--changeset`. Черновик не создаёт вторую проекцию. Если ошибка сообщает `committed=true` с оставшимся cleanup или materialization, не повторяйте изменение — выполните указанное восстановление.

Sparse commit имеет точные patches для Source add/ingest, Page put/remove, schema, purpose и записанных поисков. Веса и семантические отношения завершаются с `changeset_sparse_unsupported` до checkpoint, lock или изменения активной Wiki; применяйте их прямыми транзакциями до появления inverse patches.

## Области

| Область | Хранилище | Назначение |
| --- | --- | --- |
| `project` | Ближайший предок `.lwc/wiki.db` | По умолчанию, знания проекта |
| `global` | `~/.lwc/wiki.db` | Переиспользуемые знания |
| `all` | project и global | Только объединённые `search` и `context` |

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
```

Запись всегда явная. `all` не создаёт межхранилищные цитаты или ссылки; `search --record` только добавляет операцию в каждое выбранное хранилище.

## Поиск и CJK

Поиск лексический и детерминированный.

- Поисковые выражения — обычный текст, а не необработанный синтаксис FTS.
- `--type auto` ставит собранные страницы выше, скрывает связанные необработанные источники и оставляет источники как резервные результаты.
- Выберите `--type page`, `--type source` или `--type all`; повторяйте `--kind` для фильтра.
- Многосимвольные CJK-запросы используют соседние bigrams; полезные unigrams поддерживают один символ.
- Текст на латинице разбивается на строчные буквенно-цифровые токены.
- Заголовок, имя файла, path/slug, summary и body оцениваются отдельно; заголовки и пути получают ограниченную прибавку к оценке.
- README, индексы, обзоры и навигационные узлы понижаются в зависимости от запроса; явный запрос README снимает штраф.
- Кандидаты могут получить прибавку к оценке за прямую ссылку или общий источник. Только общие соседи порядок не меняют; слишком общие навигационные узлы штрафуются.
- `--explain` возвращает точную арифметику лексических, общих, графовых, ручных весов и feedback. Только `--record` сохраняет запрос.
- Фиксированные коэффициенты и «меньше = лучше» делают project/global сопоставимыми в `--scope all`.

Словарь сегментации намеренно не используется, чтобы имена продуктов, кодовые имена, смешанные и новые термины работали стабильно.

### Явные веса и feedback

Вес документа нужен для долговечной оценки вне запроса, feedback — для точного отпечатка упорядоченных токенов:

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

Значения: `-2`, `-1`, `1`, `2`; `clear` означает ноль. Оба механизма только переставляют лексических кандидатов. `user-provided` выше `agent-observed`, но обе строки проверяемы. Feedback хранит SHA-256, не запрос, и не переносится на перефразирование. Причины и операции долговечны: не копируйте чувствительный запрос в `--reason`. Изменения требуют `project` или `global`; `--scope all` отклоняется.

## Просмотр только для чтения и CodeGraph

`lwc view` запускает на переднем плане инспектор проекта, доступный только через локальный loopback-интерфейс, и открывает браузер. Встроенное приложение TS + Lit не требует CDN или Node во время работы и предоставляет только GET/HEAD API. Страницы, источники, Markdown, граф знаний и необязательный граф кода читаются из проекта без миграции, обновления или построения:

```bash
lwc view
lwc view --port 4173 --no-open
```

Интерфейс открывается на английском. Переключатель `中文` / `EN` меняет язык; браузер запоминает выбор, а содержимое Wiki остаётся на исходном языке. Графы используют общую 3D-визуализацию в духе Obsidian: маленькие узлы, постоянные подписи, тонкие связи, вращение и масштабирование.

<p align="center"><img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="Анализ кода LWC CodeGraph" width="100%"></p>

Индекс кода существует только в project и выключен до явной инициализации. Зафиксированная версия CodeGraph один раз скачивается из GitHub Releases, проверяется SHA-256 и кэшируется в `~/.lwc/runtime/codegraph/<PIN>/<TARGET>/`; проект хранит только индекс `.lwc/codegraph`. Телеметрия всегда выключена, состояние `.codegraph` не используется.

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

Зафиксированная версия среды выполнения распознаёт следующие языки и форматы кода: TypeScript, TSX, JavaScript, JSX, ArkTS, Python, Go, Rust, Java, C, C++, C#, Razor, PHP, Ruby, Swift, Kotlin, Dart, Svelte, Vue, Astro, Liquid, Pascal, Scala, Lua, Luau, Objective-C, R, Solidity, Nix, YAML, Twig, XML, `.properties`, CFML, CFScript, CFQuery, COBOL, VB.NET, Erlang и Terraform. YAML, Twig и `.properties` отслеживаются на уровне файла; обработчики фреймворков могут добавлять отношения. XML используется для извлечения мапперов MyBatis.

`lwc cg` перенаправляет все запросы CodeGraph. Глобальные команды жизненного цикла (`install`, `uninstall`, `upgrade`, `telemetry`, `daemon`, `daemons`) заблокированы. Мост `lwc cg serve --mcp` сохранён для старой ручной совместимости; новые интеграции используют `lwc serve --mcp`, объединяя ограниченное исследование Wiki и CodeGraph в одном инструменте только для чтения. LWC управляет средой выполнения и удерживает границу проекта. Первичная, инкрементальная, полная запись, обновление, удаление, разрешение ссылок и восстановление полностью фиксируют каждый затронутый файл перед следующим; текущий граф доступен для чтения, исторические ревизии не обновляются.

## Обслуживание и проекция

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

- Команды обслуживания сразу возвращают долговечный `work`. Читайте `work status` или ждите `work watch`, затем проверяйте `work.result`. Миграция schema v10-v11 использует тот же механизм; обычные команды не выполняют её внутри основного процесса.
- `lint` по умолчанию только читает. Добавляйте `--record`, только если проверка должна войти в историю.
- `maintenance reindex` восстанавливает производные поисковые данные из SQLite.
- `maintenance materialize` восстанавливает дерево Markdown.
- `maintenance compact` только пытается выполнить WAL truncate checkpoint, не скрывая полную FTS-оптимизацию. Запускайте при простое Wiki и проверяйте `busy` и `after_bytes`. Занятый процесс чтения быстро возвращает управление без изменения канона.
- Поисковые запросы по умолчанию приватны; `--record` сохраняет текст в долговечном журнале.

`lwc checkpoint create <NAME>` использует API оперативного резервного копирования SQLite. Восстановление — `lwc checkpoint restore <NAME>`; сначала создаётся защитный `pre-restore-*`, затем перестраивается проекция. `source remove <ID>` и `page remove <SLUG>` обеспечивают защищённое удаление: цитируемые источники и страницы с входящими ссылками отклоняются. Удаление текущего источника отслеживаемого пути прекращает отслеживание, не выставляя старую ревизию как текущую.

Для нескольких источников или широкой замены предпочитайте changeset ручному checkpoint: commit пишет sparse inverse patch, публикует только затронутые канонические сущности одной транзакцией и инкрементально материализует Markdown. Затем пытается усечь WAL; `wal_checkpointed=false` означает активный процесс чтения, а не неудачу канонического commit.

Для внешней резервной копии остановите активные `lwc` и скопируйте весь `.lwc/`. Не копируйте только `wiki.db`, пока процесс записи может использовать WAL.

## Набор бенчмарков

Необязательный benchmark импортирует локальный UTF-8 корпус во временную Wiki и измеряет время импорта, P50/P95 поиска, Recall@5/10, MRR и хранилище до/после compact. Эталон — JSONL запросов и ожидаемых относительных путей:

```bash
cargo build --release
LWC_BENCH_CORPUS=/path/to/sanitized-corpus \
LWC_BENCH_QUERY_SET=/path/to/query-set.jsonl \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
cargo test --test search_benchmark -- --ignored --nocapture
```

`cargo test --all-targets` покрывает page-first поиск, фильтры type/kind, UTF-8 окна, условия ingest, точность графа, миграции, lint и WAL compact. Контракт и правила честного сравнения — в [benchmarks/README.md](benchmarks/README.md).

## Ограничения и нецели

Текущие ограничения:

- база знаний для одной машины и одного пользователя;
- текстовый процесс UTF-8;
- до 64 МиБ на schema, purpose, source или тело страницы;
- лексический поиск, не семантическое векторное извлечение.

Намеренно не входят в цели:

- встроенные вызовы LLM;
- векторная база;
- daemon или фоновый сервис;
- Web или desktop UI;
- прямое редактирование базы данных.

Если проекция Markdown разошлась, перестройте её. Ошибки схемы SQLite исправляйте через CLI и миграции, а не вручную.

## Участие в разработке

Issues и pull requests приветствуются, особенно по темам:

- удобство процессов агентов;
- детерминированная проекция;
- долговечные контракты цитирования и поддержки страниц;
- качество поиска в многоязычных технических корпусах.

Перед pull request прочитайте [CONTRIBUTING.md](CONTRIBUTING.md). О проблемах безопасности сообщайте по [SECURITY.md](SECURITY.md).

## Лицензия

Проект распространяется по [Apache License 2.0](LICENSE).
