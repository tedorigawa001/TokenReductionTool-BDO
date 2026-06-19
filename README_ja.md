<p align="center">
  <img src="logo.svg" alt="BDO (Bushido) Logo" width="600">
</p>

<p align="center">
  <strong>LLM トークン消費を 60-90% 削減する高性能 CLI プロキシ</strong>
</p>

<p align="center">
  <a href="https://github.com/tedorigawa001/TokenReductionTool/actions"><img src="https://github.com/tedorigawa001/TokenReductionTool/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/tedorigawa001/TokenReductionTool/releases"><img src="https://img.shields.io/github/v/release/tedorigawa001/TokenReductionTool" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
</p>

<p align="center">
  <a href="https://github.com/tedorigawa001/TokenReductionTool">ウェブサイト</a> &bull;
  <a href="#インストール">インストール</a> &bull;
  <a href="docs/guide/resources/troubleshooting.md">トラブルシューティング</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">アーキテクチャ</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_ja.md">日本語</a>
</p>

---

bdo はコマンド出力を LLM コンテキストに届く前にフィルタリング・圧縮します。単一の Rust バイナリ、依存関係ゼロ、オーバーヘッド 10ms 未満。

## トークン節約（30分の Claude Code セッション）

| 操作 | 頻度 | 標準 | bdo | 節約 |
|------|------|------|-----|------|
| `ls` / `tree` | 10x | 2,000 | 400 | -80% |
| `cat` / `read` | 20x | 40,000 | 12,000 | -70% |
| `grep` / `rg` | 8x | 16,000 | 3,200 | -80% |
| `git status` | 10x | 3,000 | 600 | -80% |
| `cargo test` / `npm test` | 5x | 25,000 | 2,500 | -90% |
| **合計** | | **~118,000** | **~23,900** | **-80%** |

## インストール

### Homebrew

```bash
brew tap tedorigawa001/tap && brew install bdo
```

### クイックインストール（Linux/macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/tedorigawa001/TokenReductionTool/refs/heads/master/install.sh | sh
```

### Cargo

```bash
cargo install --git https://github.com/tedorigawa001/TokenReductionTool
```

### 確認

```bash
bdo --version   # "bdo 0.42.2" と表示されるはず
bdo gain        # トークン節約統計が表示されるはず
```

## クイックスタート

```bash
# 1. Claude Code 用フックをインストール（推奨）
bdo init --global

# 2. Claude Code を再起動してテスト
git status  # 自動的に bdo git status に書き換え
```

## 仕組み

```
  bdo なし：                                       bdo あり：

  Claude  --git status-->  shell  -->  git          Claude  --git status-->  Bushido  -->  git
    ^                                   |             ^                      |          |
    |        ~2,000 tokens（生出力）     |             |   ~200 tokens        | フィルタ |
    +-----------------------------------+             +------- （圧縮済）----+----------+
```

4つの戦略：

1. **スマートフィルタリング** - ノイズを除去（コメント、空白、ボイラープレート）
2. **グルーピング** - 類似項目を集約（ディレクトリ別ファイル、タイプ別エラー）
3. **トランケーション** - 関連コンテキストを保持、冗長性をカット
4. **重複排除** - 繰り返しログ行をカウント付きで統合

## コマンド

### ファイル
```bash
bdo ls .                        # 最適化されたディレクトリツリー
bdo read file.rs                # auto: 軽い整理と大きいソースのスマート短縮
bdo read file.rs -l none        # 正確な全文が必要なとき
bdo read file.rs -l aggressive  # 強めの整理（boilerplate をより多く除去）
bdo read file.rs -l outline     # シグネチャのみ — 全 fn/struct/trait、本体は省略
bdo map src/                    # リポジトリ地図: ディレクトリ配下全ファイルのトップレベル署名
bdo smart file.rs               # 2行の技術要約
bdo find "*.rs" .               # コンパクトな検索結果
bdo grep "pattern" .            # ファイル別グループ化検索
```

`find`/`grep` は常に総数を表示し、表示が打ち切られた場合は `+N more — use --all …` 行を出すため、マッチが無言で落ちることはありません。`--all` で全上限（件数および grep の per-file 上限）を解除します。

#### コードマップ（`bdo map`）

ディレクトリ全体の API 全体像を一発で取得します。各ファイルのトップレベル宣言だけを示し、関数本体は省略。全ファイルを読む（=トークンを払う）ことなく、エージェントに新しいコードベースを把握させるのに最適です。

```console
$ bdo map src/core
runner.rs
  pub fn run(cmd: Command, tool_name: &str, args_display: &str, mode: RunMode<'_>, opts: RunOptions<'_>) -> Result<i32> { … }
  pub struct RunOptions<'a> { … }
  pub enum RunMode<'a> { … }
stream.rs
  pub trait StreamFilter { … }
  pub fn run_streaming(cmd: &mut Command, stdin_mode: StdinMode, stdout_mode: FilterMode<'_>) -> Result<StreamResult> { … }
…
— 16 files, 245 signatures (full source: 9,188 lines)
```

このリポジトリでは **約74,000トークンのソースを約3,500トークンで表現**（約95%削減）し、トップレベル API は完全に保持します。Rust / Go / JS・TS / C / C++ / Java / Python に対応、`.gitignore` を尊重します。

`bdo map --changed` は git の変更セットだけにマップを絞ります（`--against origin/main` でブランチ全体の差分）。触った API 面だけを見られ、`bdo review` と併用すると便利です。

Python では関数本体を `…` マーカーで省略（`{ … }` に相当）、`async def` も `def` と同様に処理、複数行シグネチャは1行に折りたたみます:

```python
async def run(task: Task, retries: int = 3) -> Result: …
class Config: …
```

#### 変更レビュー（`bdo review`）

変更セットを人間 + エージェント向けに一発要約します。`git status`・`rg`・`cargo test` を手で組み合わせていた作業を1コマンドに:

```console
$ bdo review                 # 作業ツリーの変更（既定）
$ bdo review --against origin/main   # ref に対するブランチ全体の差分

bdo review — 3 changed file(s) (uncommitted)

CHANGED
  M  src/core/filter.rs
  ?? src/cmds/system/review.rs

⚠ ARTIFACTS (0)
  ✓ none
⚠ STALE MARKERS (1) — verify before commit
  scripts/x.sh:27  broken install URL (blob serves HTML)

🧪 SUGGESTED TESTS
  cargo test -- filter review
```

紛れ込んだ生成物（`__pycache__`・`target/`・`.bak` 等）、高シグナルな stale マーカー（旧名・壊れた install URL）、変更 Rust ファイルに対応する inline テストモジュールを検出します。

#### 残骸監査（`bdo stale`）

`bdo review` が変更セットを見るのに対し、`bdo stale` は **tracked tree 全体**の残骸（git に紛れ込んだ生成物・旧名・壊れた install URL 等）を監査し、見つかれば**非ゼロ終了**します（CI ゲートに利用可）。`bdo stale <path>` でスコープ可。

### Git
```bash
bdo git status                  # コンパクトなステータス
bdo git log -n 10               # 1行コミット
bdo git diff                    # 圧縮された diff
bdo git push                    # -> "ok main"
```

### テスト
```bash
bdo jest                        # Jest コンパクト
bdo vitest                      # Vitest コンパクト
bdo pytest                      # Python テスト（-90%）
bdo go test                     # Go テスト（-90%）
bdo test <cmd>                  # 失敗のみ表示（-90%）
bdo test --changed              # 変更した Rust ファイルのテストだけ実行（+ --against <ref>）
```

`bdo test --changed` は git の変更セットから `cargo test -- <stems>`（ファイル stem = inline テストモジュール。例: `src/core/outline.rs` → `outline`）を導出し、対象だけを失敗のみ表示で実行します。`bdo review` / `bdo map --changed` と併用すると便利です。

### ビルド & リント
```bash
bdo lint                        # ESLint ルール別グループ化
bdo tsc                         # TypeScript エラーグループ化
bdo cargo build                 # Cargo ビルド（-80%）
bdo ruff check                  # Python リント（-80%）
```

### 分析
```bash
bdo gain                        # 節約統計
bdo gain --graph                # ASCII グラフ（30日間）
bdo discover                    # 見逃した節約機会を発見
```

## ドキュメント

- **[troubleshooting.md](docs/guide/resources/troubleshooting.md)** - よくある問題の解決
- **[INSTALL.md](INSTALL.md)** - 詳細インストールガイド
- **[ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md)** - 技術アーキテクチャ

## ライセンス

Apache 2.0 ライセンス - 詳細は [LICENSE](LICENSE) を参照。

## 免責事項

詳細は [DISCLAIMER.md](DISCLAIMER.md) を参照。
