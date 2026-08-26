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

LWC は、永続的な知識を追跡可能に保つため、次の層を分離します。

| レイヤー | 役割 |
| --- | --- |
| Raw sources | 選別された証拠の不変スナップショット |
| Wiki | エージェントが保守するページ、引用、リンク、provenance |
| Schema と purpose | 今後の知識保守を導くプロジェクト固有の規則 |

SQLite が正本です。Markdown、全文検索インデックス、任意のグラフストアは再構築
可能な投影であり、操作結果は監査と復旧に適した構造化 JSON で返されます。

[アーキテクチャの詳細 →](https://github.com/JanYork/llm-wiki-cli/wiki/Architecture-Overview)

## 階層型の想起とナレッジグラフ

LWC は Source と Wiki ページを文書、パッセージ、文の粒度で索引化します。
エージェントは小さく関連性の高いコンテキストから始め、必要な箇所だけを展開できます。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-memory-graph-en.png" alt="LWC のメモリグラフ" width="100%">
</p>

任意の文書グラフは、ページ、Source、引用、リンク、明示的な意味関係を接続します。
SQLite は常に正本であり、Grafeo または SurrealDB は再構築可能な探索レイヤーです。
関係には理由、provenance、信頼度、根拠となる Source が保持されます。

### 文書変換と Office 読み取り

任意の Anydoc または MarkItDown アダプターは、対応するローカルファイルを確認可能な
Markdown に変換してから取り込みます。OfficeCLI は Word、Excel、PowerPoint 用の
独立した同意制・読み取り専用経路です。どちらも暗黙には導入・有効化されず、元の
Office ファイルを変更しません。

[検索と索引 →](https://github.com/JanYork/llm-wiki-cli/wiki/Retrieval-and-Indexing) ·
[文書グラフ →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Knowledge-Graph) ·
[文書変換 →](https://github.com/JanYork/llm-wiki-cli/wiki/Document-Conversion)

## インストール

多くの利用者に必要なのは、次の 1 コマンドだけです。

    npm install --global @i-xor/lwc

Homebrew、crates.io、チェックサム検証済み GitHub Release、ローカル Cargo
ビルドにも対応しています。

[インストールと更新 →](https://github.com/JanYork/llm-wiki-cli/wiki/Installation-and-Upgrades)

## エージェント向け Skill

同梱の [using-lwc Skill](../../skills/using-lwc) は、LWC をプロアクティブな
メモリ層として利用できるようにします。範囲を限定したコンテキストを想起し、
project と global の知識を分離し、Source と引用を保守し、再利用に値する検証済み
知識だけを書き戻します。

[skills.sh](https://skills.sh/JanYork/llm-wiki-cli) から導入できます。

    npx skills add JanYork/llm-wiki-cli --skill using-lwc -g

標準の呼び出しは <code>$using-lwc</code> です。Skill は特定のエージェントに依存せず、
メモリ、文書グラフ、Word Graph、CodeGraph、strong tag、文書変換、導入、復旧、
保守のガイドを含みます。

### ネイティブなエージェント設定

LWC は対応エージェントを検出し、AgentTarget アダプターを通じて利用可能な MCP、
Skill、Hook、Instructions を冪等に設定します。

    lwc agent install --yes

統合された読み取り専用 MCP は、ワークスペース境界を広げずに Wiki メモリと任意の
コードコンテキストを提供します。Claude Code、Codex、Cursor、OpenCode、
Gemini CLI、Kiro、Hermes、Antigravity、pi に対応します。

[AgentTarget 連携 →](https://github.com/JanYork/llm-wiki-cli/wiki/AgentTarget-Installation-and-Integration)

## クイックスタート

通常、人は目的を伝えて結果を確認し、CLI の操作はエージェントに任せます。完全な
手順は [Quick Start Wiki](https://github.com/JanYork/llm-wiki-cli/wiki/Quick-Start)
にあります。

### 1. プロジェクト Wiki を初期化する

エージェントはプロジェクトローカルの Wiki を作成し、purpose と保守規則を定義します。
明示的にバージョン管理を選ばない限り、状態は Git のローカル除外に追加されます。

### 2. 出典資料を追加する

選別したファイルは、重複排除された不変スナップショットになります。LWC は元の
パスを追跡し、現在のファイルが未変更、変更済み、欠落、更新済みかを判定できます。

### 3. 1 件の source を分析して統合する

エージェントは範囲を限定した完全な Source を読み、引用付き要約を作成し、共有知識を
更新し、両方の層が整合してから取り込みを完了します。

### 4. 蓄積した Wiki を検索する

検索は保守済みページを優先し、出典とのつながりを維持します。主張の検証が必要な
場合にだけ、エージェントが正確な原文を開きます。

## エージェントのワークフロー

通常の流れは、関連知識の想起、必要な最新 Source やコードの確認、最小限の検証済み
更新、検索・リンク・該当グラフ投影の検証です。大規模な更新は changeset で原子的に
公開します。

[完全なワークフロー →](../../docs/agent-workflow.md)

## 複数コマンドを原子的に変更する

Changeset は、複数段階の知識更新をレビューと検証が終わるまで非公開に保ちます。
commit は触れた正本エンティティだけを 1 トランザクションで公開し、無関係な変更を
保持します。同じエンティティの revision が競合した場合は安全に失敗します。

対応する操作では正確な逆パッチが保存されるため、Wiki 全体を置き換えずに保護された
rollback が可能です。

[Changesets の詳細 →](https://github.com/JanYork/llm-wiki-cli/wiki/Changesets)

## スコープ

| スコープ | 用途 |
| --- | --- |
| project | 最も近いプロジェクト Wiki が所有する知識 |
| global | プロジェクト間で再利用する知識 |
| all | 読み取り専用の統合想起と協調 Sync |

書き込み先は常に 1 つのストアとして明示され、暗黙のクロスプロジェクト引用や
リンクは作成されません。

[スコープとプロジェクト検出 →](https://github.com/JanYork/llm-wiki-cli/wiki/Scopes-and-Project-Discovery)

## 検索と CJK

検索は語彙ベースで決定的に動作し、保守済みページを優先します。タイトル、パス、
要約、本文、provenance、グラフ証拠を分けて評価し、page/source/kind フィルターと
スコア内訳の説明に対応します。

CJK には隣接 bigram と有用な unigram、ラテン文字には小文字の英数字 token を使います。
辞書に依存しないため、製品名、コード記号、混在言語、新しい語彙にも安定して対応します。

### 明示的な検索 weight と feedback

監査可能な文書 weight は長期的な重要度を表し、query 固有の feedback は一致する
候補だけを再順位付けします。生の query ではなく fingerprint を保存し、無関係な
文書を結果へ追加することはありません。

[検索とコンテキスト →](https://github.com/JanYork/llm-wiki-cli/wiki/Search-and-Context)

## 読み取り専用 Viewer と CodeGraph

ローカル Viewer は loopback 限定の読み取り専用インターフェースで、ページ、Source、
Markdown、文書関係、コード構造を表示します。migration、refresh、graph 構築は
行いません。

<p align="center">
  <img src="https://raw.githubusercontent.com/JanYork/llm-wiki-cli/main/docs/images/lwc-codegraph-en.png" alt="LWC CodeGraph のコードインテリジェンス" width="100%">
</p>

CodeGraph は project 専用で、明示的に初期化します。symbol、caller、callee、依存、
file、影響範囲を調べられ、telemetry は常に無効、graph 更新は owner file 単位で
原子的です。

固定 runtime は TypeScript、TSX、JavaScript、JSX、ArkTS、Python、Go、Rust、
Java、C、C++、C#、Razor、PHP、Ruby、Swift、Kotlin、Dart、Svelte、Vue、
Astro、Liquid、Pascal、Scala、Lua、Luau、Objective-C、R、Solidity、Nix、
YAML、Twig、XML、.properties、CFML、CFScript、CFQuery、COBOL、VB.NET、
Erlang、Terraform を認識します。

[Viewer →](https://github.com/JanYork/llm-wiki-cli/wiki/Read-Only-Viewer) ·
[CodeGraph →](https://github.com/JanYork/llm-wiki-cli/wiki/Code-Graph)

## 保守と投影

lint、索引再構築、Markdown materialize、compact、checkpoint、graph projection は
明示的な操作です。長時間処理は永続的に追跡、監視、再開でき、文書単位で実行されます。

SQLite は常に正本です。検索索引、Markdown、グラフストアは Source の履歴や現在の
Wiki 知識を書き換えずに再構築できます。

[保守と診断 →](https://github.com/JanYork/llm-wiki-cli/wiki/Maintenance-and-Diagnostics)

## ベンチマークスイート

任意の benchmark は、利用者が用意したサニタイズ済み corpus 上で import 時間、
search latency、Recall@5/10、MRR、storage を測定します。公平な比較では machine、
corpus、query set、実行条件を固定し、複数回の中央値を使います。

[ベンチマーク方法 →](../../benchmarks/README.md)

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
