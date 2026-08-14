<h1 align="center">LWC — AIエージェントのためのプロアクティブメモリ</h1>

<p align="center">
  <strong>エージェント主導 · 永続化 · 出典に基づく</strong>
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@i-xor/lwc"><img alt="npm: @i-xor/lwc" src="https://img.shields.io/badge/npm-%40i--xor%2Flwc-CB3837?logo=npm"></a>
  <a href="https://crates.io/crates/lwc"><img alt="crates.io: lwc" src="https://img.shields.io/crates/v/lwc.svg"></a>
  <img alt="Node.js 22 以降" src="https://img.shields.io/badge/node-%3E%3D22-5FA04E?logo=nodedotjs">
  <img alt="対応プラットフォーム: macOS、Linux、Windows" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-666666">
  <a href="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/JanYork/llm-wiki-cli/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://skills.sh/janyork/llm-wiki-cli/using-lwc"><img alt="skills.sh: using-lwc" src="https://img.shields.io/badge/skills.sh-using--lwc-000000?logo=vercel"></a>
  <a href="../../LICENSE"><img alt="ライセンス: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
  <a href="../../README.md">English</a> · <a href="../../README.zh-CN.md">简体中文</a> ·
  <a href="README.ja.md">日本語</a> · <a href="README.es.md">Español</a> ·
  <a href="README.pt-BR.md">Português (Brasil)</a> · <a href="README.fr.md">Français</a> ·
  <a href="README.ru.md">Русский</a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-social-preview.png" alt="LWC — AIエージェントのためのプロアクティブメモリ" width="100%">
</p>

`lwc` は、AIエージェント向けのエージェント主導型プロアクティブメモリ CLI です。
セッションをまたいで、永続的で出典を追跡できる知識をエージェント自身が呼び出し、
保守し、発展させられます。

**Claude Code、Codex、Cursor、OpenCode、Gemini CLI、Kiro、Hermes、
Antigravity、pi に対応しています。**

LWC は、選別した文書を長期運用できる Wiki に変換します。推論と統合はエージェントが
担い、`lwc` は出典、ページ、引用、リンク、インデックス、履歴を保存します。
問い合わせのたびに未加工の断片から調べ直すのではなく、知識を積み重ねられます。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-overview-en.png" alt="LWC の概要" width="820">
</p>

## LWC は RAG ではなく、エージェントの記憶です

RAG と LWC はどちらも LLM が外部文書を扱う助けになりますが、状態を残す場所が
異なります。一般的な RAG は、問い合わせごとに未加工のチャンクを検索し、その場で
回答を生成します。

```text
query -> retrieve chunks -> generate answer
```

LWC は、すでに行った有用な作業を次の問い合わせにも残します。

```text
task -> recall maintained Wiki -> reason from sources and prior synthesis
     -> write durable improvements back
```

検索は LWC の一機能であり、全体を規定する仕組みではありません。中心となる成果物は、
知識の変化に合わせてページ、引用、リンク、矛盾、履歴を更新し続ける、出典付きの
Wiki です。そのため、LWC は埋め込みやベクトルデータベースを必要とせず、回答後に
統合結果を捨てることもありません。RAG と併用できますが、LWC 自体は問い合わせ時に
完結する RAG ではありません。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-source-grounding-en.png" alt="LWC の出典管理と追跡可能性" width="820">
</p>

### LWC を操作するのはエージェントです

`lwc` は人間向けのノートアプリではなく、エージェント向けの機械インターフェースです。
通常、人間は出典を選び、目的と質問を示し、回答や投影された Markdown を確認します。
エージェントは CLI を実行し、スコープを管理し、出典を統合し、引用とリンクを保守し、
何を呼び出し、何を書き戻す価値があるかを判断します。

ツール自体を開発またはデバッグする場合を除き、日常の `lwc` ワークフローを人間が
手作業で進める必要はありません。代わりに、エージェントへ同梱の標準
`using-lwc` Skill を有効にするよう依頼してください。通常は `$using-lwc` で呼び出せます。

## 推奨: エージェントに LWC のセットアップを任せる

次のプロンプトを、普段使っているエージェントに貼り付けてください。グローバル CLI を
インストールし、対応済みホストの設定は冪等な AgentTarget インストーラーに任せます。
未登録のエージェントに限り、そのホスト固有の正式な仕組みで設定します。

<details>
<summary><strong>セットアップ用プロンプトをすべてコピー</strong></summary>

```text
このユーザー向けに LWC を完全にセットアップしてください。手順を説明するだけでなく、
実際に作業して検証してください。

信頼できる情報源:
- https://github.com/JanYork/llm-wiki-cli
- https://github.com/JanYork/llm-wiki-cli/tree/main/skills/using-lwc

要件:
1. この README、`SECURITY.md`、`skills/using-lwc/SKILL.md` を読んでください。
   `lwc` をグローバルに呼び出せない場合は、チェックサム検証済みの公式リリースを
   インストールしてください。通常のコマンドに非公開のバイナリパスや
   `LWC_PROJECT_ROOT` を付けないでください。
2. `lwc --version` を実行し、グローバルメモリがない場合だけ
   `lwc --scope global init` で一度初期化してから、`lwc agent install --yes` を
   実行してください。このコマンドはインストール済みの対応エージェントを検出し、
   公式の場所へ MCP、Skill、Hook、Instructions を安全に導入します。この処理を
   手作業で作り直したり、同じエージェントへネイティブパッケージも併用したりしないでください。
3. `lwc agent status --target all --location global` を確認してください。必要な
   エージェントを再起動し、ホスト側で通常求められる Hook の信頼確認を完了してください。
   プロジェクトの明示的な同意なしに、プロジェクト Wiki やグラフを初期化しないでください。
4. 現在のランタイムが LWC の登録済み AgentTarget でない場合に限り、そのランタイムの
   公式なユーザーレベル規約に従って、標準 `using-lwc` Skill、追記型の指示ブロック、
   `lwc serve --mcp`、および正式対応している場合だけ境界付きセッション Hook を設定して
   ください。既存設定を保持し、冪等性を保ってください。未対応の機能は、パスやキーを
   推測せず未対応と報告してください。

最後に、LWC のバージョン、検出・設定した Target、status の結果、変更したファイル、
未対応の機能、残っている再起動または信頼確認を報告してください。
```

</details>

## 着想と謝辞

`lwc` は Andrej Karpathy が提案した
[LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) の
パターンを実装しています。問い合わせのたびに未加工文書から知識を組み直すのではなく、
LLM が永続的で相互にリンクされた Wiki を少しずつ構築し、保守していく考え方です。
CLI の構成と一部の実装は、
[`nashsu/llm_wiki`](https://github.com/nashsu/llm_wiki) からも着想を得ています。

本プロジェクトは、これらの考え方を SQLite ベースのエージェントファーストな Rust CLI
として実用化しています。

## 基本設計

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-architecture-en.png" alt="LWC のアーキテクチャ" width="100%">
</p>

永続知識モデルは、論理的に次の 3 層で構成されます。

| 層 | 内容 | 契約 |
| --- | --- | --- |
| Raw sources | 選別した入力の不変スナップショット | `source` から追加し、出典の原文は書き換えない。 |
| Wiki | エージェントが保守するページ、引用、リンク、来歴 | `page` から更新し、出典を引用して、長期保存する非出典知識を分類する。 |
| Schema and purpose | 保守ルールとプロジェクトの目的 | 以降の ingest と改訂すべてを導く。 |

正本は SQLite です。Markdown ツリーは、人間や Obsidian などのツール向けに再構築
できる投影です。エージェントは `.lwc/wiki.db` や投影済み Markdown を直接編集せず、
`lwc` を通じて知識を変更します。成功したコマンドは stdout に JSON、失敗した
コマンドは stderr に構造化 JSON を返します。

現行形式のストアに対する読み取りコマンドは、ストアを変更しません。新しい CLI が
書き込み可能な旧形式ストアを初めて開くときだけ、読み取り前に一度、トランザクションで
スキーマを移行します。

## 階層型の想起とナレッジグラフ

現在有効な Source と Wiki ページは、決定的な方法でパッセージと文に索引付けされます。
SQLite が引き続き正本であり、span FTS と任意の外部文書グラフは再構築可能な
インデックスです。粒度を指定しない既存検索は、文書だけを返します。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="LWC のメモリグラフ" width="100%">
</p>

```bash
lwc search "projection consistency" --granularity sentence --type page
lwc search "projection consistency" --granularity passage
lwc search "projection consistency" --granularity all --group-by document
lwc span get <SPAN_ID>
lwc span expand <SPAN_ID> --before 1 --after 1 --children 20
```

span locator には文書フィンガープリントと分割バージョンが含まれます。本文を置換すると、
古い locator は `stale_span` で失敗し、新旧のメタデータを返します。LWC が似た文章へ
暗黙に割り当て直すことはありません。

キーワードなしで探索する場合は、境界付きの型付きグラフ API を使います。

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

自動生成されるエッジは、構造または証拠で確認できる事実に限られます。意味上の関係は、
明示的かつ監査可能でなければなりません。

```bash
lwc graph relation set page:implementation DEPENDS_ON page:policy \
  --provenance source-grounded --source 12 \
  --reason "Source 12 states the required policy" --confidence 0.95
lwc graph relation list --from page:implementation
lwc graph relation retract page:implementation DEPENDS_ON page:policy \
  --reason "The dependency was superseded"
```

関係の理由は長期保存されます。認証情報、秘密情報、生の思考過程を含めないでください。

SQLite 文書が常に正本です。グラフ保存は既定で無効です。走査が必要なときだけ、外部
エンジンを 1 つ選んで有効にしてください。設定は、組み込みの既定値、グローバル、
プロジェクトの順に重ねて解決されます。

```bash
lwc config show
lwc config set --graph grafeo
lwc config set --graph surrealdb
lwc config set --graph disabled
lwc config unset --graph
```

Markdown 変換は別のオプトイン操作です。`lwc init` は同じ機械可読の案内を返しますが、
変換ツールをインストールしたり有効にしたりはしません。アダプターを 1 つインストールして
明示的に選び、新しいローカル Markdown ファイルへ変換して内容を確認してから ingest します。

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

設定では `--trans-timeout 1..900` と、選択したアダプターへ渡す複数の
`--trans-arg=<value>` を指定できます。LWC は選択済みの実行ファイルだけを直接起動し、
別のアダプターへ自動で切り替えません。入力はローカルファイルに限り、入出力はそれぞれ
64 MiB までで、既存の出力を上書きしません。認証情報は LWC 設定ではなく、
アダプターの環境変数に置いてください。対応形式とオプションは、公式の
[Anydoc](https://github.com/firecrawl/anydoc) および
[MarkItDown](https://github.com/microsoft/markitdown) の文書を参照してください。

Grafeo と組み込み SurrealDB は、`.lwc/` 配下の破棄可能な sidecar を使います。
各 `graph-project` Work は、現在の Source/Page 1 件と、それが所有するリンク、引用、
明示的な関係を完全にコミットしてから次の文書へ進みます。更新と削除は、実際に変更した
文書だけをキューへ追加します。再構築と再開も同じ文書単位です。過去の source リビジョンは
不変で、再トークン化も再投影もしません。進捗は `work list`、`work status`、
`work watch` で確認し、中断後は `work resume` を使います。`graph status` は選択中の
エンジンと投影文書数を返し、`graph verify` は現在の文書キーを SQLite と照合します。

## インストール

多くのユーザーには、前述のエージェント向けセットアッププロンプトを推奨します。
以下の手動コマンドは、保守、デバッグ、または同梱 Skill を導入できない環境向けです。

Homebrew でインストールします。Apple silicon macOS と x86_64 Linux にはビルド済み
Bottle があります。

```bash
brew install JanYork/tap/lwc
```

npm でインストールします（Node.js 22 以降）。

```bash
npm install --global @i-xor/lwc
```

crates.io からインストールします。

```bash
cargo install --locked lwc
```

GitHub からインストールします。

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | sh
```

インストーラーは x86_64/aarch64 の macOS、glibc Linux、Windows Git Bash に対応し、
リリースのチェックサムを検証して `lwc` をインストールまたは更新します。既定では
`~/.local/bin` へインストールし、`~/.local/bin` または `~/.cargo/bin` に既存の
コピーがあれば更新します。別のディレクトリを選ぶ場合は次のように実行します。

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/JanYork/llm-wiki-cli/releases/latest/download/install.sh | LWC_INSTALL_DIR="$HOME/bin" sh
```

Cargo で GitHub からビルドしてインストールすることもできます。

```bash
cargo install --locked --git https://github.com/JanYork/llm-wiki-cli
```

ローカルのリポジトリコピーからインストールする場合は次のとおりです。

```bash
git clone https://github.com/JanYork/llm-wiki-cli.git
cd llm-wiki-cli
cargo install --locked --path .
```

## エージェント向け Skill

このリポジトリには [`skills/using-lwc`](../../skills/using-lwc) が含まれています。
これは、内容のあるセッションで `lwc` をプロアクティブメモリ層として使うための
Agent Skill です。[skills.sh](https://skills.sh/JanYork/llm-wiki-cli) から導入できます。

```bash
npx skills add JanYork/llm-wiki-cli --skill using-lwc -g
```

ローカルのリポジトリコピーから、使用中のエージェントランタイムのユーザーレベル Skill
ディレクトリへコピーすることもできます。Codex の場合は次のとおりです。

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/using-lwc "$HOME/.agents/skills/"
```

標準の呼び出し名は `$using-lwc` です。

Skill が有効になると、次の処理を行います。

- 互換 CLI を探し、なければチェックサム検証済みの公式リリースをインストールする。
- `~/.lwc/` のグローバルメモリを一度だけ初期化する。
- 同じ調査を繰り返す前に、境界付きのグローバルおよびプロジェクト文脈を呼び出す。
- 明示的に呼び出された場合は現在のプロジェクトを初期化し、それ以外は先に確認する。
- 現在許可されたワークスペースルート外へのプロジェクト書き込みを拒否する。
- プロジェクト固有の事実と、再利用できるグローバル知識を分ける。
- 出典を統合し、長期保存する回答を Wiki へ書き戻す。

`SKILL.md` は巨大な手引きではなく、短いルーターです。基本メモリ、有効化の判断、
アクティブメモリ、物理文書グラフ、境界付き Word Graph、CodeGraph、strong tag、
文書変換、エージェントのオンボーディング、復旧・保守について、目的別の文書へ
案内します。各文書には、利用すべき場合と省略すべき場合、最小手順、同意の境界、
完了を示す証拠が明記されています。

Skill は通常、現在のディレクトリから対象プロジェクトを検出し、グローバルに
インストールされた `lwc` を直接呼び出します。`LWC_PROJECT_ROOT` は、意図的に
別のプロジェクト境界を指定するときだけ使います。すでに作業中のプロジェクトで、
日常コマンドの接頭辞として設定するものではありません。

自動インストールを無効にするには `LWC_AUTO_INSTALL=0` を設定します。自動
インストールは Skill 同梱のレビュー済みインストーラーを実行し、このリポジトリと
GitHub Release の公開境界を信頼したうえで、ダウンロードしたアーカイブを
`SHA256SUMS` と照合します。チェックサムは完全性を守るもので、公開者のコード署名
ではありません。リリースバイナリは x86_64/aarch64 macOS、glibc Linux、Windows
Git Bash に対応します。`SKILL.md` は Agent Skills のリソース配置に従い、
`agents/openai.yaml` は OpenAI/Codex 向けメタデータを提供します。CLI 自体は
ランタイムに依存しません。CLI を実行し、Skill の指示を読み込むか適合させられる
エージェントなら LWC を利用できます。一方、Skill コマンド、グローバル指示、Hook
の登録方法はランタイム固有なので、セットアッププロンプトが現在のホストを検出して
設定します。

### ネイティブなエージェント設定

LWC は対応エージェントを検出し、統合された読み取り専用 LWC MCP を導入できます。
登録済みの 12 AgentTarget はすべて強いアダプターです。各ホストとスコープで公式に
提供されているファイルベースの MCP、Skill、Hook、Instructions をすべて導入し、
UI 管理、プレビュー、未対応の機能は明示して報告します。

```bash
lwc agent install --yes
lwc agent status --target all --location global
lwc agent install --print-config codex
lwc agent refresh --target codex,claude
lwc agent uninstall --target codex,claude --yes
```

`--yes` は検出済みエージェント、グローバルスコープ、各 Target の既定の
ライフサイクル・プロンプト Hook を選びます。Claude のプロンプトごとの Hook を
省くには `--no-prompt-hook` を使います。導入されるエントリは
`lwc -> serve --mcp` です。唯一の `lwc_explore` ツールは、既定で境界付き Wiki
メモリを読み取り、明示的な `code` / `all` モードも受け付けます。要求する
`projectPath` は、MCP ホストが LWC を起動したワークスペース内に限られます。
問い合わせ中に CodeGraph をダウンロードしたり初期化したりはしません。install と
refresh を繰り返してもバイト単位で冪等です。uninstall は LWC が所有する状態だけを
元に戻し、プロジェクトインデックスを残します。Codex、Claude Code、Pi 向けの任意
パッケージは `integrations/` にあります。パッケージを導入してもネイティブな信頼を
付与・回避することはなく、同じエージェントへ直接インストーラーとネイティブ
パッケージを併用しないでください。各パッケージは完全な `using-lwc` Skill を同梱し、
外部 Skill マネージャーや保守担当者固有の環境に依存しません。

Pi には組み込み MCP がないため、公式の拡張ブリッジ経由で LWC MCP を公開します。
その他の Target が登録するのは `lwc serve --mcp` だけです。CodeGraph は LWC 内部の
コード文脈プレーンであり、2 つ目の MCP として公開しません。ホスト UI が所有する
信頼・権限設定はユーザー管理のままです。プレビュー機能にはその旨を表示し、
プロジェクトスコープで一部だけ対応できる場合は、Target 全体を弱めたり拒否したりせず、
対応部分を導入します。Kiro のグローバルパスは `KIRO_HOME` に従います。

Target インターフェース、レジストリ順、検出規則、MCP パスは、MIT ライセンスの
CodeGraph インストーラー用アダプター設計に基づいています。LWC はその上に、統合
LWC MCP、機能ごとの状態報告、Skills、Hooks、共有ファイルの所有権、正確な
ロールバックを追加しています。詳しくは
[`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md) を参照してください。

新規プロジェクトの `lwc init` 出力と、セッション開始・圧縮時の Hook は、境界付きの
`LWC_READINESS` 情報を提供します。Wiki、物理文書グラフ、CodeGraph ランタイムと
プロジェクトインデックス、エージェント連携コマンドが含まれます。物理グラフの準備
状態では、設定済みの同意と、投影の保留・失敗を区別します。検出は読み取り専用で、
グラフを有効化または初期化しません。両グラフに承認が必要な場合、共通の基本形は
プレーンテキストです。チェックボックスに対応しないエージェントでも同じように動作します。

```text
1. Enable physical document graph and CodeGraph (recommended)
2. Enable physical document graph only
3. Enable CodeGraph only
4. Later
```

ユーザーが `1` を明示的に選ぶと、エージェントは必要に応じてプロジェクト Wiki を
初期化し、Grafeo を有効化し、その投影 Work を待って検証し、CodeGraph を初期化して、
両方の結果を個別に確認します。`Later` は何も変更せず、主な作業も妨げません。
ネイティブプラグインは同じ選択 ID を独自 UI で表示できますが、チェックボックスを
必須とはしません。

strong tag は、主要ルールやランブックを検索せず、少数のページだけ全文読み込むための
境界付き機能です。

```bash
lwc tag set "operations" incident-response --priority 100 --reason "primary runbook"
lwc load tag "operations" --limit 3
lwc tag autoload "operations" --enable --priority 100 --limit 3 \
  --max-chars 50000 --reason "required at session boundaries"
```

これは token から派生する検索ではありません。ページ全文をエージェントの文脈へ
入れる前に、件数と文字数の上限を適用します。

## クイックスタート

この節では、エージェントが実行する CLI プロトコルを説明します。通常の利用で、
人間がこれらのコマンドを実行する必要はありません。

### 1. プロジェクト Wiki を初期化する

```bash
cd your-project
lwc init
printf '# Schema\nEvery page declares provenance; source-grounded claims cite sources.\n' | lwc schema set -
printf '# Purpose\nBuild a durable project Wiki.\n' | lwc purpose set -
```

必要であれば、初期化時にプロジェクト相対の `.lwc/` パスを Git のローカル
`info/exclude` へ追加します。リポジトリの `.gitignore` は変更しません。
Wiki 自体を意図的にバージョン管理する場合だけ `lwc init --no-git-exclude` を使います。

### 2. 出典資料を追加する

```bash
lwc source add-dir docs/
```

明示的なタイトルがないファイルには、安定して読めるフォールバックとして出典の origin
を使います。同じバイト列は SHA-256 で重複排除されます。プロジェクトの有効な Wiki
ルート外へ解決される出典には `--allow-external-source` が必要です。信頼度の高い
認証情報マーカーは、内容をレビューして `--acknowledge-sensitive-source` で明示的に
承認しない限り拒否されます。

追加に成功すると、確認したファイルパスと、その時点の不変スナップショットも記録します。
ファイル由来の証拠を使う前に、今回の作業に必要な出典だけを確認してください。

```bash
lwc source status 7 12
```

このコマンドは各 live ファイルを SHA-256 へストリーミングし、パスの系譜
（`current` または `superseded`）と、ファイルシステムの状態（`current`、
`modified`、`missing`、`unreadable`、`oversized`、`unstable`）を分けて返します。
読み取り専用です。`source status --all` のコストは追跡中の全ファイルのバイト数に
比例するため、明示的な保守作業だけで使ってください。変更されたパスは、知識を更新する
前に確認します。

```bash
lwc source diff 7
lwc source refs 7 --limit 1000
```

`source diff` は不変 source と live ファイルを比較します。`--to-source` を使えば別の
スナップショットとも比較できます。統合 diff は、各側 8 MiB・200,000 行まで、出力は
既定で Unicode 20,000 文字、`--max-chars` 指定時は 100,000 文字までです。1 つの
source が複数のパスで観測されている場合は、正確な `--path` を指定してください。
途中で切れた diff はプレビューにすぎません。`source refs` は直接引用しているレビュー
候補を列挙しますが、意味上影響を受けるページを証明するものではありません。同じパスに
意味のある新リビジョンがあるとレビューできた場合だけ、`source add` を再実行します。
A -> B -> A と変化した場合、内容 A は元の source ID を再利用しても、パス観測は 3 回分
残ります。外部 live パスには再び `--allow-external-source` が必要で、live テキストが
機密情報チェックに該当した場合は、確認後に `--acknowledge-sensitive-source` も必要です。

旧ストアから移行した source は、LWC が過去のパスを推測しないため、明示的に未追跡の
ままです。対象ファイルを一度追加し直すと、最初の追跡リビジョンができます。確認中に
ファイルまたはパスの最新 revision が変わると `source_status_unstable` を返します。異なる時点の
結果を信用せず、再実行してください。

選別済みの資料を原子的に追加する場合、JSON manifest 内のパスは manifest の
ディレクトリを基準に解決されます。

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

### 3. 1 件の source を分析して統合する

```bash
lwc ingest next --context-limit 50 --source-max-chars 100000
lwc ingest analyze 1 --file analysis.md
```

manifest またはスケジューラーが保留中の source ID をすでに選んでいる場合は、
`lwc ingest claim 7` を使います。

`source_window.has_more` が true なら、`source_window.next_offset_chars` から
続きを読みます。

```bash
lwc source show 1 --offset-chars 100000 --max-chars 100000
```

ingest を完了する前に、引用付きの source-summary ページを作り、その source の
寄与を少なくとも 1 件の非 source ページへ統合します。

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

両方の層が必要です。source ページはナビゲーションと来歴を助け、非 source ページは
知識を積み重ねられる形にします。本当に共有ページへ変更をもたらさない source は、
監査できる具体的な理由を付けて完了します。

```bash
lwc ingest complete 1 \
  --no-derived-pages-reason "Duplicate evidence; existing synthesis already covers every supported claim"
```

source の引用があるページには、`source-grounded` 来歴が自動で付きます。長期知識が
ユーザーの説明、エージェントの観察、明示的な仮説に由来する場合は、架空の source を
作らず、必要なだけ `--provenance` を繰り返してください。

```bash
lwc page put architecture-decision \
  --title "Architecture decision" \
  --kind query \
  --summary "Accepted constraint and remaining uncertainty" \
  --file decision.md \
  --provenance user-provided \
  --provenance hypothesis
```

`page put` は、引用と明示的な出所情報の集合全体を置き換えます。既存ページを
先に読み、現在も有効な `--source` と source 由来でない `--provenance` をすべて再指定して
ください。`source-grounded` は引用から導出されるため、明示的に渡さないでください。
出所情報はページ読み取り、context、search、source refs、Markdown 投影に返されますが、
検索順位は変えません。

### 4. 蓄積した Wiki を検索する

```bash
lwc context --limit 50
lwc search "question keywords" --limit 20
lwc search "question keywords" --limit 20 --explain
lwc search "concept only" --type page --kind concept
lwc search "exact evidence" --type source
lwc page show source-1
```

## エージェントのワークフロー

想定する流れは次のとおりです。

1. 不変の source を集める。
2. 境界付きの `lwc ingest next` で ingest タスクを 1 件 claim する。source が明示的に
   選ばれている場合は `ingest claim <ID>` を使う。
3. 返された source window をすべて読み、schema、purpose、境界付き context も読む。
4. ページを生成する前に分析する。
5. 明示的な `--source` 引用を付け、source summary と共有の長期知識ページを作成または改訂する。
6. 2 つの統合ゲートを満たしてから完了する。共有ページを変更しない場合は理由を記録する。
7. 複数コマンドの ingest や広範な改訂は 1 つの changeset に入れ、草稿を検証してから原子的に公開する。
8. `search`、`context`、`graph`、`lint` で Wiki の整合性を保ち続ける。

完全な運用契約は [docs/agent-workflow.md](../../docs/agent-workflow.md) にあります。
エージェント向けの前提条件、状態遷移、副作用、次の操作は、`lwc --help` または
`lwc <command> --help` で確認できます。

## 複数コマンドを原子的に変更する

単一の `source` または `page` コマンドはトランザクションです。1 つの論理変更に
複数のコマンドが必要で、途中の Wiki を公開できない場合は changeset を使います。

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

草稿の読み取りには準備済みの変更が見えますが、正式な SQLite と Markdown は変わりません。
草稿データベースは軽量な sparse overlay として始まり、正式な Wiki のコピーも checkpoint
も作りません。`changeset show` は lint を実行せず、staged operation、revision、
準備状態を報告します。commit は変更対象だけを検証して適用するため、無関係な正式
書き込みは残ります。同じエンティティの revision が衝突した場合は、どちらも上書きせず失敗
します。空の草稿と lint issue は拒否され、強制または自動 merge はありません。
changeset が追加していない、レビュー済みの既存負債に限り、
`--allow-lint-issues --reason "reviewed pre-existing debt"` を使えます。commit 後は、
同じ固定検索を live state に対して再実行してください。公開前にレビュー済み草稿を
freeze し、以降の staged write は `changeset_frozen` で拒否されます。復旧時は同じ
commit を再実行するか、報告された衝突後に破棄してください。freeze 済みの草稿へ
作業を追加してはいけません。

```bash
lwc --scope project changeset discard architecture-refresh
lwc --scope project changeset rollback <CHANGESET_ID>
```

discard が触るのは未 commit の草稿だけです。commit は変更対象だけを含むチェックサム
付き inverse patch を書き込み、正確な rollback ID を返します。rollback はそのエンティティ
だけを復元し、その後に再変更されたエンティティがあれば拒否します。project と global の
changeset は別々で、`--scope all` は無効です。`init`、`maintenance`、`checkpoint`、
入れ子の changeset コマンドは `--changeset` を拒否します。草稿が 2 つ目の Markdown
投影を作ることはありません。構造化エラーが `committed=true` を返し、cleanup または
materialization だけが残っている場合は、知識変更を繰り返さず、返された復旧操作を
実行してください。

sparse commit が現在、正確な patch を持つのは Source add/ingest、Page put/remove、
schema、purpose、記録付き search operation です。検索 weight と明示的な semantic
relation の変更は、checkpoint、正式 Wiki の write lock、正式 Wiki の変更より前に
`changeset_sparse_unsupported` で失敗します。sparse inverse patch が実装されるまでは、
直接の単一エンティティトランザクションとして適用してください。

## スコープ

`lwc` には 3 つのスコープがあります。

| スコープ | ストア | 用途 |
| --- | --- | --- |
| `project` | 最も近い祖先の `.lwc/wiki.db` | 既定。プロジェクト固有の知識 |
| `global` | `~/.lwc/wiki.db` | プロジェクト間で再利用する知識 |
| `all` | project と global の両方 | 統合した `search` と `context` のみ |

例:

```bash
lwc --scope global init
lwc --scope global source add shared.md
lwc --scope all search "shared term"
lwc --scope all context
```

知識の書き込み先は常に明示します。`all` がストア間の引用やリンクを暗黙に作ることは
ありません。`search --record` は、選択した各ストアに query operation だけを追加します。

## 検索と CJK

検索は語彙ベースで決定的です。

- 検索語はプレーンテキストで、未加工の FTS 構文ではありません。
- 既定の `--type auto` は、編集済みページを優先し、対応する raw source を隠し、
  raw source をフォールバックの想起に使います。
- レイヤーは `--type page`、`--type source`、`--type all` から選びます。
  `--kind concept --kind synthesis` のように `--kind` を繰り返せば、ページ種別を限定できます。
- 複数文字の CJK 検索語には隣接 bigram を使います。1 文字の検索にも対応できるよう、
  stopword でない unigram も保持します。
- ラテン文字は小文字の英数字 token に分割します。
- タイトル、source filename、path/slug、summary、body の証拠を別々に評価します。
  タイトルとパスの完全・部分一致には境界付き boost を与えます。
- README/index/overview 文書と明示的なナビゲーションハブは、具体的な機能文書を
  優先するため query に応じて downweight します。README や overview 自体を求める
  query では penalty を無効にします。
- ページ候補には、境界付きの直接リンクまたは共有 source による graph boost が付きます。
  common neighbor だけの関係は順位を変えず、広すぎるナビゲーションハブには境界付き
  graph penalty を適用します。
- `--explain` は lexical、generic、graph、manual-weight、query-feedback を含む
  正確な score 計算を返します。query は記録せず、履歴へ残すのは `--record` だけです。
- 固定係数と「小さいほど上位」の rank により、`--scope all` で project と global の
  結果を比較できます。

意図的に辞書へ依存していません。外部分かち書き辞書を使わず、製品名、コードネーム、
複数言語が混ざった用語、新しい語彙でも安定した挙動を得るためです。

### 明示的な検索 weight と feedback

query に依存しない長期的なページ・source 評価には document weight を使います。
特定の順序付き token query fingerprint には feedback を使います。

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

文書の値は `-2`、`-1`、`1`、`2` で、0 に戻す場合は `clear` を使います。
どちらも、すでに語彙検索に一致した候補を並べ替えるだけで、不一致の文書を出現させる
ことはできません。`user-provided` の行は `agent-observed` より優先されますが、両方とも
監査用に残ります。feedback が保存するのは元の query ではなく SHA-256 fingerprint で、
token が異なる言い換えには引き継がれません。理由と operation record は長期保存される
ため、機密 query を `--reason` にコピーしないでください。変更には明示的な `project`
または `global` scope が必要で、`--scope all` は拒否されます。

## 読み取り専用 Viewer と CodeGraph

`lwc view` は、loopback 限定のプロジェクトインスペクターを foreground で起動し、
ブラウザーを開きます。組み込みの TS + Lit アプリ 1 つを配信するため、利用時に CDN
も Node ランタイムも必要ありません。API は GET/HEAD だけです。ページ、source、
Markdown、ナレッジグラフ、任意のコードグラフを、移行・refresh・graph construction
なしで現在のプロジェクトから読み取ります。

```bash
lwc view
lwc view --port 4173 --no-open
```

viewer の初期表示は英語です。画面内の `中文` / `EN` で言語を切り替えられ、選択は
ブラウザーに保存されます。Wiki 本文は作成時の言語のままです。ナレッジグラフと
コードグラフは、Obsidian に着想を得た共通の 3D 関係ビューを使い、小さな node、
常時表示 label、細い link、回転、zoom を提供します。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="LWC CodeGraph のコードインテリジェンス" width="100%">
</p>

コードの index は project scope 専用で、明示的に初期化するまで無効です。LWC 固定の
CodeGraph fork は GitHub Release から 1 度だけダウンロードし、SHA-256 で検証して
`~/.lwc/runtime/codegraph/<PIN>/<TARGET>/` にキャッシュします。各プロジェクトが
保持するのは `.lwc/codegraph` の index だけです。telemetry は常に無効で、
`.codegraph` state は使いません。

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

固定ランタイムは、次の言語とコード関連形式を認識します。TypeScript、TSX、
JavaScript、JSX、ArkTS、Python、Go、Rust、Java、C、C++、C#、Razor、PHP、Ruby、
Swift、Kotlin、Dart、Svelte、Vue、Astro、Liquid、Pascal、Scala、Lua、Luau、
Objective-C、R、Solidity、Nix、YAML、Twig、XML、`.properties`、CFML、CFScript、
CFQuery、COBOL、VB.NET、Erlang、Terraform。YAML、Twig、`.properties` はファイル
単位で追跡し、framework resolver が関係を補う場合があります。XML は MyBatis mapper
の抽出に使います。

CodeGraph の query 機能はすべて `lwc cg` が転送します。グローバル lifecycle command
（`install`、`uninstall`、`upgrade`、`telemetry`、`daemon`、`daemons`）は拒否します。
正確な `lwc cg serve --mcp` bridge は従来の手動互換性のために残しています。新しい
エージェント連携は `lwc serve --mcp` を使い、1 つの読み取り専用ツールで境界付き Wiki
と CodeGraph の探索を提供します。ランタイムの所有とプロジェクト境界の強制は LWC が
担います。初回、増分、全量、更新、削除、参照解決、復旧の各書き込みは、所有者ファイルを
1 件ずつ完全に commit してから次へ進みます。現在の graph は読み取り可能なままで、
過去の文書 revision は更新しません。

## 保守と投影

よく使う保守コマンド:

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

注意点:

- maintenance command は、永続 `work` をすぐに返します。`work status` で進捗を読み、
  または `work watch` で待って、成功後に `work.result` を確認してください。schema v10
  から v11 の移行も自動的に同じ仕組みを使うため、通常コマンド内では移行しません。
- `lint` は既定で読み取り専用です。lint の実行自体を永続 operation history に残す
  必要がある場合だけ `--record` を付けます。
- `maintenance reindex` は SQLite から派生検索データを再構築します。
- `maintenance materialize` は SQLite から投影 Markdown ツリーを再構築します。
- `maintenance compact` は WAL truncate checkpoint だけを試み、完全な FTS 最適化を
  隠れて行いません。Wiki が idle の間に実行し、`busy` と `after_bytes` を確認して
  ください。読み取り処理が busy なら、正本を変更せずすぐに戻ります。
- 検索 query は既定で非公開です。文言を永続 operation log へ保存したい場合だけ
  `--record` を付けてください。

`lwc checkpoint create <NAME>` は SQLite の online backup API を使います。復元には
`lwc checkpoint restore <NAME>` を使います。LWC は最初に安全用の `pre-restore-*`
checkpoint を作り、その後で投影を再構築します。保護付き削除には
`source remove <ID>` と `page remove <SLUG>` を使います。引用のある source や、
inbound link のあるページは拒否されます。追跡パスの current source を削除すると、
古い revision を current として暗黙に公開せず、そのパスの追跡を停止します。

複数 source の ingest または広範なページ置換には、手動 checkpoint より changeset を
推奨します。成功した commit は sparse inverse patch を書き、変更対象の正本エンティティ
だけを 1 transaction で公開し、変更済み Markdown を増分 materialize します。
公開後に WAL truncate を試みます。`wal_checkpointed=false` は稼働中の読み取り処理により
実行できなかったことを示すだけで、正本への commit 失敗ではありません。

外部ファイルシステムへバックアップする場合は、稼働中の `lwc` command を停止して、
`.lwc/` ディレクトリ全体をコピーしてください。書き込み処理が WAL ファイルを使っている可能性が
ある状態で、`wiki.db` だけをコピーしてはいけません。

## ベンチマークスイート

任意実行の benchmark は、ローカル UTF-8 corpus を一時 Wiki へ import し、import 時間、
search P50/P95、Recall@5/10、MRR、compact 前後の storage を報告します。正解データ
には、query と期待する corpus-relative path を記した JSONL を使います。

```bash
cargo build --release
LWC_BENCH_CORPUS=/path/to/sanitized-corpus \
LWC_BENCH_QUERY_SET=/path/to/query-set.jsonl \
LWC_BENCH_BINARY="$PWD/target/release/lwc" \
cargo test --test search_benchmark -- --ignored --nocapture
```

通常の `cargo test --all-targets` は、page-first search、type/kind filter、UTF-8 source
window、ingest completion gate、graph precision、migration、lint、WAL compact を
対象にします。workload contract と公平な前後比較の規則は
[benchmarks/README.md](../../benchmarks/README.md) を参照してください。

## 制約と対象外

現在の設計上の制約:

- 単一マシン・単一ユーザーのナレッジベース。
- UTF-8 テキストのワークフロー。
- schema、purpose、source、page body 1 件あたり 64 MiB の入力上限。
- semantic vector retrieval ではなく lexical search。

この CLI で意図的に対象外としているもの:

- 組み込みの LLM 呼び出し。
- ベクトルデータベース。
- daemon または background service。
- Web UI または desktop UI。
- データベースを直接編集する契約。

投影済み Markdown にずれが生じたら再構築してください。SQLite schema に問題がある
場合は、手作業ではなく CLI と migration で修正してください。

## コントリビューション

issue と pull request を歓迎します。特に次の領域への貢献を求めています。

- エージェントワークフローの使いやすさ。
- 決定的な投影動作。
- 長期引用とページ保守の契約。
- 多言語技術 corpus の検索品質。

pull request を作る前に [CONTRIBUTING.md](../../CONTRIBUTING.md) をお読みください。
security issue は [SECURITY.md](../../SECURITY.md) に従って報告してください。

## ライセンス

[Apache License 2.0](../../LICENSE) の下で提供します。
