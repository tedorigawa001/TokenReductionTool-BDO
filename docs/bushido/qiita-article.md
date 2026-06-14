<!--
Qiita 投稿用の下書き。そのまま貼り付け可。
推奨タグ: Rust, CLI, LLM, ClaudeCode, 生成AI
-->

# AIエージェントのトークンを6〜9割減らすCLI「bdo」を作った

## これは何

AIコーディングエージェント（Claude Code など）は `git status` や `cat` の**生出力をそのままコンテキストに飲み込む**ため、トークンを無駄に使います。

**bdo (Bushido)** はコマンドとLLMの間に挟まる**フィルタ・プロキシ**。出力を圧縮してから渡すことで **60〜90% のトークン削減**をします。

```
従来:  Claude → git status → 生出力 2,000 tokens
bdo:   Claude → git status → [bdoが圧縮] → 200 tokens
```

実測では代表11コマンド合計で **155,000 → 6,000 トークン（約96%減）**、コードベース全体の把握は **約95%減**（後述）。情報を落とさずに、です。

[rtk (Rust Token Killer)](https://github.com/rtk-ai/rtk)（Apache-2.0）のフォークで、堅牢化と新機能を加えた「武士道版」です。
リポジトリ → https://github.com/tedorigawa001/TokenReductionTool

## どう動く

フックで `git status` → `bdo git status` に**自動で書き換え**ます。普段どおりコマンドを打つだけ。危険・非対応コマンド（`rm -rf` など）は素通しです。

## 使い方（3ステップ）

> ⚠️ リリース前のため、今はソースからビルドします（要 Rust）。

```bash
# 1. インストール
git clone https://github.com/tedorigawa001/TokenReductionTool
cd TokenReductionTool && cargo install --path .

# 2. フック設置（Claude Code）
bdo init -g

# 3. Claude Code を再起動 → 以降コマンドが自動で bdo 経由に
```

あとは普通に使うだけ。`bdo gain` で削減量を確認できます。

## どれくらい減る（実測）

このリポジトリ自身で計測（同梱の `scripts/bushido-token-benchmark.sh`）。

| コマンド | raw tok | bdo tok | 削減 |
|---|--:|--:|--:|
| `grep fn src`（3,502行ヒット） | 68,427 | 34 | **99.9%** |
| `smart`（コード要約） | 27,619 | 29 | **99.9%** |
| `cargo check`（エラー時） | 2,166 | 25 | **98.8%** |
| `cat`（read, 3,284行） | 27,619 | 2,430 | **91.2%** |
| `git log -10 --stat` | — | — | **81%** |
| `find *.rs` | 691 | 186 | **73%** |
| `ls -la` | 433 | 123 | **71%** |
| `git status` | 175 | 94 | 46% |
| `git status --short` | 77 | 77 | 0%（既に最小） |

➡ **代表11ケース合計: 155,068 → 6,087 トークン（≈96%削減）**

しかも `cargo check` はエラー診断（`error[E0308]` の file:line・コード・キャレット）を**1文字も削らず保持**したまま、`Compiling…` などのノイズだけ落とします。LLM はそのまま修正に使えます。

### コードベース把握はさらに劇的に（`bdo map`）

| 対象 | 全文 | bdo map | 削減 |
|---|--:|--:|--:|
| `src/core`（16ファイル / 9,188行） | ~74,000 tok | ~3,500 tok | **≈95%** |
| リポジトリ全体（126ファイル / 75,895行） | — | 3,344 署名 | — |

トップレベル API は**完全保持**。「`ls` してから各ファイルを読む」何往復ものやり取りが、**1コマンド・約3,500トークン**に置き換わります。

### 1セッション（約30分）の試算

| 操作 | 標準 | bdo | 削減 |
|---|--:|--:|--:|
| `cat`/`read` ×20 | 40,000 | 12,000 | -70% |
| `cargo test` ×5 | 25,000 | 2,500 | -90% |
| `grep` ×8 | 16,000 | 3,200 | -80% |
| `git` 各種 | 約17,000 | 約2,000 | 約-88% |
| **合計** | **~118,000** | **~23,900** | **-80%** |

> ※この表は中規模プロジェクト想定の試算（上記の実測ベンチとは別）。

## 武士道版で足した機能

- **`bdo read -l outline`** — 関数の中身を省き、**シグネチャだけ**表示。ファイルのAPI把握用。
- **`bdo map <dir>`** — ディレクトリ配下**全ファイルのトップレベル署名を一発**で。
  例: `src/core`（約9,200行）が **74,000 → 3,500 トークン（≈95%減）**、API は完全保持。
- **`bdo curl`** — JSONを minify（端末時のみ）。`| shasum` や `> file` などパイプ/リダイレクトは**バイトそのまま**。
- **fail-safe 修正** — フィルタが落ちても raw 出力にフォールバック（プロセスごと落ちない）。

```console
$ bdo map src/core
runner.rs
  pub fn run(cmd: Command, ...) -> Result<i32> { … }
  pub struct RunOptions<'a> { … }
…
— 16 files, 245 signatures (full source: 9,188 lines)
```

## 正直な注意点

- `cat`/`head` も `bdo read` に化けるので、**生の全文が欲しい時**は `bdo read -l none`。
- 対象は **Bashツール呼び出しのみ**（Claude Code の `Read`/`Grep` は書き換わらない）。
- 既に短い出力（`git status --short` 等）は**無理に削りません**（正しさ優先）。

## クレジット

[rtk (Rust Token Killer)](https://github.com/rtk-ai/rtk)（Apache-2.0）のフォークです。設計・大半の実装は上流に帰属し、ライセンスも Apache-2.0 を継承しています。
