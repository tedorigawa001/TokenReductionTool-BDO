<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="Bushido - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>LLM トークン消費を 60-90% 削減する高性能 CLI プロキシ</strong>
</p>

<p align="center">
  <a href="https://github.com/tedorigawa001/TokenReductionTool/actions"><img src="https://github.com/tedorigawa001/TokenReductionTool/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/tedorigawa001/TokenReductionTool/releases"><img src="https://img.shields.io/github/v/release/tedorigawa001/TokenReductionTool" alt="Release"></a>
  <a href="https://opensource.org/licenses/Apache-2.0"><img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg" alt="License: Apache 2.0"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1478373640461488159?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://github.com/tedorigawa001/TokenReductionTool">ウェブサイト</a> &bull;
  <a href="#インストール">インストール</a> &bull;
  <a href="docs/guide/resources/troubleshooting.md">トラブルシューティング</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">アーキテクチャ</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
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

### Homebrew（推奨）

```bash
brew install bdo
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
bdo --version   # "bdo 0.27.x" と表示されるはず
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
bdo read file.rs -l aggressive  # シグネチャ中心（実装本体を省略）
bdo smart file.rs               # 2行の技術要約
bdo find "*.rs" .               # コンパクトな検索結果
bdo grep "pattern" .            # ファイル別グループ化検索
```

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
```

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
