# Bushido Maintenance Plan

この文書は、Bushido を武士道チームで保守・改造していくための開始台帳です。
上流の設計思想を尊重しつつ、武士道環境で安全に使い続けられる状態を目指します。

## 現状

- プロジェクト本体は Rust 製 CLI です。
- 中核は `src/main.rs`, `src/cmds/`, `src/core/`, `src/hooks/` です。
- 現在の作業ディレクトリには `.git` ディレクトリがなく、Git履歴は確認できません。
- 現在のシェルでは `cargo`, `rustc`, `rustup` が見つからないため、Rustのビルド検証は未実行です。

## 保守方針

1. まずは武士道版の目的を「LLM作業時の出力圧縮・自動フック・利用状況分析」に絞ります。
2. 上流 rtk の原則である correctness, transparency, never block, zero overhead は維持します。
3. 変更は小さく分け、各変更ごとにテストまたは手動確認コマンドを残します。
4. ブランド変更、デフォルト設定変更、フック挙動変更は、コード修正より先に影響範囲を記録します。
5. ローカル運用で必要な機能を優先し、上流追従しやすい形で差分を抑えます。

## 最初に固めること

### 1. ビルド環境

Rust toolchain を入れて、最低限この3つが通る状態にします。

```bash
cargo check
cargo test
cargo fmt --all --check
```

その後、変更前ベースラインとして以下を記録します。

```bash
cargo test --all
cargo clippy --all-targets
bash scripts/check-test-presence.sh
```

### 2. Git 管理

このコピーには `.git` がないため、武士道版として保守するなら最初にGit管理を開始します。
上流追従を考える場合は、上流URLと取得元のバージョンをこの文書に追記します。

記録する項目:

- 上流リポジトリ
- 取得日
- 取得バージョン
- 武士道版の初期タグ
- 上流追従方針

### 3. 武士道版の差分

初期改造候補:

- README / install 文言を武士道運用向けに調整する
- デフォルトのフック対象エージェントを武士道で使うものに寄せる
- `bdo gain` など分析出力に、武士道向けの簡易サマリを追加する
- プロジェクトローカルの推奨設定 `.rtk/` を整える
- 日本語ドキュメントを最新構成に合わせて修正する

## 優先ロードマップ

### Phase 0: 検証できる状態にする

- Rust toolchain を導入する
- `cargo check` を通す
- `cargo test` の失敗有無を確認する
- `.git` 管理を開始するか、上流からcloneし直すかを決める

### Phase 1: 保守しやすい武士道版にする

- READMEのリンク切れや古い説明を直す
- `docs/bushido/` に運用メモを集約する
- よく使う確認コマンドを `scripts/bushido-check.sh` として用意する

### Phase 2: 使うエージェントに寄せる

- Codex / Claude / Cursor / Copilot のうち、実際に使うフックだけ重点検証する
- `bdo rewrite` の対象コマンドを武士道の作業パターンに合わせて調整する
- 除外コマンドや危険コマンドの扱いを明文化する

### Phase 3: 武士道独自機能を足す

- 日本語ログや日本語エラーの圧縮品質を上げる
- よく使うツール向けの TOML フィルタを追加する
- 分析コマンドに日次・週次の武士道運用サマリを追加する

## 変更時のチェックリスト

- 変更範囲は1テーマに絞ったか
- 既存の `cmds/`, `core/`, `hooks/` の境界を崩していないか
- 失敗時に元コマンドの実行を妨げないか
- verbose指定時に必要な詳細を見られるか
- トークン削減より正しさを優先しているか
- テストまたは手動確認コマンドを記録したか

## 実施記録

### 2026-06-13 — fail-safe 修正 + bdo リネーム（フェーズ1: コマンド層）

ブランチ: `bushido/rebrand-and-failsafe`。検証: `cargo build` / `cargo test`（2154 passed, 0 failed, 8 ignored）+ 実バイナリのスモークテスト。

**A. fail-safe 修正（設計原則 #4 の実効化）**
- `Cargo.toml`: release profile を `panic = "abort"` → `panic = "unwind"`。abort 下では `catch_unwind` が無効で、フィルタの panic がプロセスごと出力を巻き込んでいた。
- `src/core/runner.rs`: 捕捉フィルタ呼び出しを `catch_unwind` で包み、panic 時は raw 出力にフォールバック（50/54 のコマンドが該当）。
- `src/core/stream.rs`: ストリーミングフィルタ（cargo/tsc/gradlew 等の重いパーサ4種）の `feed_line` / `flush` / `on_exit` を `catch_unwind` で保護。panic 後は raw パススルーへ degrade し、exit code を保持。
- 回帰テスト `test_run_streaming_filter_panic_falls_back_to_raw` を追加。

**B. コマンド名リネーム（crate `bushido` / バイナリ `bdo` / 表示名 Bushido）**
- `Cargo.toml`: `name = "bushido"`、`[[bin]] name = "bdo"`、deb/rpm のアセットパスを `bdo` に。
- clap の `name`/`about`/`long_about` を Bushido に（上流帰属「fork of rtk (Rust Token Killer)」は保持）。
- コマンド生成・検出（`discover/rules.rs` の `rtk_cmd`、`registry.rs` の検出/生成）、tracking ラベル、`[rtk]`→`[bdo]` ログ接頭辞を更新。
- バイナリに同梱（`include_str!`）されるエージェント連携テンプレートのコマンド/バイナリ呼び出しを `bdo` に（pi/opencode の `exec("bdo", ...)`、hermes の `which("bdo")` / `["bdo","rewrite",...]` 等）。

### 2026-06-13 — フェーズ2: 完全リネーム（RTK 残さない方針）

検証: `cargo build` / `cargo test`（2154 passed, 0 failed, 8 ignored）。RTK_ 互換は残さない（保守工数削減）。

- **env 変数**: `RTK_*` → `BDO_*` を全面置換（実 env 読取り12箇所 + 子プロセス用マーカー + `BDO_DISABLED=` 接頭辞検出 + `option_env!("BDO_TELEMETRY_URL/TOKEN")`）。互換フォールバックなし。
- **データ/設定ディレクトリ**: `~/.local/share/rtk` → `~/.local/share/bdo`（`BDO_DATA_DIR` 定数値 + ハードコードの `.join("rtk")` を統一）。プロジェクトローカル設定 `.rtk/` → `.bdo/`。旧データはクリーン切替で非参照（移行フォールバックなし）。
- **フックのファイル名・マーカー**: `rtk.ts`→`bdo.ts`、`rtk-awareness.md`→`bdo-awareness.md`、hermes プラグイン `bdo-rewrite`→`bdo-rewrite`、copilot `bdo-rewrite.json`→`bdo-rewrite.json`、`rtk-hook-gemini.sh`→`bdo-hook-gemini.sh`、`rtk-rules.md`→`bdo-rules.md`、CLAUDE.md/AGENTS.md の `rtk-instructions` マーカー→`bdo-instructions`。同梱ファイル実体も `mv` 済み・`include_str!` パス更新済み。
- **非 src 資材**: `Formula/rtk.rb`→`bdo.rb`（class `Bdo`、`bin.install "bdo"`）、`install.sh`（`BINARY_NAME=bdo`、`BDO_*` env）、`scripts/rtk-economics.sh`→`bdo-economics.sh`、docs/README/各 README のコマンド・env・製品名（`RTK`→`Bushido`）を一括変換。

### 2026-06-13 — リポジトリURL差し替え

フォークの GitHub を `https://github.com/tedorigawa001/TokenReductionTool` に確定。`github.com/rtk-ai/rtk` / `rtk-ai/tap/rtk` / star-history 等の**機能的URL**を全面差し替え（Cargo.toml の homepage/repository、`install.sh` の REPO、`Formula/bdo.rb` の url/homepage/tap=`tedorigawa001/tap`、README×2・INSTALL・docs・openclaw・src 内 issues リンク）。Homebrew tap は `tedorigawa001/homebrew-tap` を前提（Formula にコメント）。

**意図的に残した rtk（レガシー処理 / 上流帰属 / 連絡先）**
- 旧フック `bdo-rewrite.sh` の**アンインストール/検出コード**（`REWRITE_HOOK_FILE` 定数等）— 上流由来の実在ファイルを掃除する処理のため。
- 連絡先メール `contact@rtk-ai.app` / `security@rtk-ai.app`（docs/TELEMETRY, SECURITY, INSTALL）— フォークの連絡先未確定のため保持（テレメトリは既定 off）。
- `LICENSE` の著作権表記、`CONTRIBUTING.md` の CLA（rtk-ai への権利付与）— 法務。フォークの方針が決まれば見直し。
- src 内の小文字内部識別子（`rtk_cmd`, `rtk_disabled_count`, `RtkStatus` 等）— 出力・env に出ないため churn 回避。
- `tests/fixtures/`・`CHANGELOG.md` の `rtk` — テスト入力データ / 上流履歴のため不変更。

### 2026-06-13 — 残課題対応（メール削除 / CLA / LICENSE / 名称衝突）

- **連絡先メール削除**: `contact@rtk-ai.app` / `security@rtk-ai.app` を全廃し、GitHub issues / security advisory URL に置換（src のテレメトリ consent/erasure 表示 `telemetry_cmd.rs`・`init.rs` 含む）。連絡は GitHub で行う方針。
- **CLA 削除**: `CONTRIBUTING.md` の CLA セクション（rtk-ai 社への権利付与・`bdo Pro`・CLA Assistant・存在しない `CLA.md` リンク）をセクションごと削除。
- **LICENSE**: 上流 `Copyright 2024 rtk-ai and rtk-ai Labs` を保持し、`Copyright 2026 tedorigawa001 (Bushido fork)` を追記（Apache-2.0 準拠）。
- **名称衝突警告の撤去**: バイナリが `rtk`→`bdo` になり reachingforthejack/rtk（Rust Type Kit, binary `rtk`）との衝突が解消したため、陳腐化した「2つの bdo がある / Type Kit と間違えるな」記述を8ファイル（README, INSTALL, CLAUDE, docs/troubleshooting, docs/installation, 同梱 bdo-awareness.md, copilot awareness, check-installation.sh）から削除/簡素化。stale バイナリパス `target/release/rtk`→`bdo`、`cargo install bdo`→`bushido`、リリース成果物名 `rtk-*`→`bdo-*` も修正。

**意図的に残した rtk**
- 旧フック `bdo-rewrite.sh` の**アンインストール/検出コード**（`REWRITE_HOOK_FILE` 定数等）。
- src 内の小文字内部識別子（`rtk_cmd`, `rtk_disabled_count`, `RtkStatus` 等）。
- `tests/fixtures/`・`CHANGELOG.md`・src のテスト入力データ（`install_method_from_path` のサンプルパス、cargo 置換フィクスチャ等）。
- `LICENSE` の上流著作権行（Apache-2.0 が保持を要求）。

### 2026-06-13 — scripts 監査（バイナリ名バグ修正 + dir/ラベル整合）

レビュー指摘を機に scripts/ を横断監査:
- **バグ修正①** `scripts/install-local.sh`: `bdo` をビルドして `${INSTALL_DIR}/rtk` に install していた（PATH 上のコマンド名が rtk になる実害）→ `${INSTALL_DIR}/bdo` に修正。
- **バグ修正②** `scripts/test-install.sh`: 偽バイナリを `safe_src/rtk` で作成後に `tar ... bdo`（不在ファイル）→ tar 失敗。`safe_src/bdo` に修正。
- **データ/出力ラベル整合**: `bushido-token-benchmark.sh`・`bushido-check.sh` の `$TEST_HOME/rtk/*`（DB/tee/data dir）→ `/bdo/`、temp dir `rtk-test-home`/`rtk-target-new` → `bdo-*`、`benchmark.sh` の出力サブディレクトリ `$BENCH_DIR/rtk` → `/bdo`、出力文言「install rtk」「data from rtk」→ bdo、ベンチVM の clone dir `/home/ubuntu/rtk` → `/home/ubuntu/bushido`・VM名 `rtk-test`→`bushido-test` 等。`bushido-token-benchmark.sh:9` の `debug/rtk`→`debug/bdo` は既修正を確認。
- **据え置き（出力/ディレクトリではない）**: 内部シェル/TS変数（`rtk_cmd`, `rtk_out`, `TOTAL_RTK`, `rtkMean`, `rtk_db` 等）、サンプル/フィクスチャ（`rtk-bench` crate、`test@rtk.dev`、path-traversal テストの `rtk/..`）、レガシー `bdo-rewrite.sh` 検出（`validate-docs.sh`, `check-installation.sh`）。
- `benchmark-sessions/lib/runner.py` が参照する `setup-rtk.sh` は非同梱（既存 dangling 参照）。

**残課題**
- GitHub に `bdo-<target>.tar.gz` リリース成果物と Homebrew tap (`tedorigawa001/homebrew-tap`) を用意。
- 上流追従が不要なら、レガシー `bdo-rewrite.sh` 掃除コードの削除を検討。
- `check-installation.sh` のフック検出が legacy `bdo-rewrite.sh` のみ＝ネイティブ `bdo hook` 方式を検出しない点の要否判断。
- 標準 python/bash プラグインテストの実行確認（`hooks/hermes/tests/`, `hooks/*/test-*.sh`）。

### 2026-06-14 — 機能追加 + レビュー駆動の修正（main 集約後）

作業は `main` に集約済み（fast-forward）。以降 `main` で開発。追加・修正:
- `feat(read)`: `-l outline`（シグネチャのみ・本体省略）、`feat(map)`: `bdo map`（リポジトリ地図、`outline::signatures` 再利用、`.gitignore` 尊重、多言語）。`feat(curl)`: JSON minify（**端末時のみ**、pipe/redirect は byte 完全 passthrough=#1282 維持）。
- レビュー修正: `map` を `RUST_HANDLED_COMMANDS` 登録、複数行シグネチャの1行正規化（`code_part` で行末コメント誤合体も修正）。
- README / README_ja: 事実誤り一掃（旧 `rtk` パス・データディレクトリ `~/.config/rtk`→`bdo`・`rtk-rules.md`→`bdo-rules.md`・hermes `bdo-rewrite/`→`bdo-rewrite/`・成果物名 `rtk-*`→`bdo-*`・version `0.2x`→`0.42.2`・壊れた `/guide/` リンク→相対 docs・ロゴ `assets/logo.svg`→root `logo.svg`・Homebrew バッジ削除）。`-l outline` と `bdo map`(専用セクション)を追記。

**リリースは現在ペンディング**（未実施）。以下のリリース依存タスクは保留:
- GitHub リリース成果物 `bdo-<target>.tar.gz` / Homebrew tap (`tedorigawa001/homebrew-tap`) の整備。
- README/install.sh/Formula の install 手順（Homebrew tap・releases・install.sh URL）はリリース後に有効。
- push / PR もリリース方針確定まで保留。

### 2026-06-17 — ドッグフーディング駆動の修正（grep / Python outline / 診断 / リネーム）

`main` で作業。`cargo test` 全 2191 passed / 0 failed / 8 ignored。release バイナリも `cargo install --path .` で再ビルド・反映済み。

- `fix(grep)`: `bdo grep -h` がフックで `--help` 化していた衝突を解消。clap の auto `-h` を無効化し `-h`/`--no-filename` を `no_filename` フラグに束縛 → bdo の再帰検索（rg/grep）へ転送。`--help`（long のみ・`ArgAction::Help`）は維持。`-l`/`-m` の short を外し grep/rg へ passthrough。→ [94f0fcf]
- `feat(outline)`: Python に本体省略マーカー `def foo(): …` を導入（Rust の `{ … }` に相当）。あわせて **`async def` を関数として認識**（未対応で本体が素通しだった）、**複数行シグネチャを 1 行に折りたたみ**（`scan_header_colon`）、ワンライナー本体/行末コメントを除去。実測 `runner.py` は outline が実質 0% → 約 84% 削減に改善。→ [fffa057]
- `fix(outline)`: map（`collapse_all`）モードで Python decorator を抑制（`@deco` が "signature" として水増しカウントされていた問題）。→ [d644cfb]
- `fix(check-installation)`: Check 6 をネイティブフック（settings.json の `bdo hook claude`）検出に修正。レガシー script 検出のみだった誤診断を解消。→ [834f753]
- `refactor(hooks)`: `rtk-rewrite.sh` → `bdo-rewrite.sh` に全面リネーム（`REWRITE_HOOK_FILE`・`.rtk-hook.sha256`→`.bdo-hook.sha256`・テストスクリプト `test-rtk-rewrite.sh`→`test-bdo-rewrite.sh`・コード/docs/フィクスチャ）。リポジトリ全体で `rtk-rewrite` 文字列ゼロ。**メンテナ判断: 上流 rtk からの移行検知は廃止**（`rtk-rewrite.sh` の検出/掃除はしない）。→ [72df662]
- `fix(check-installation)`: stale 箇所を修正。壊れた install URL（`blob`=HTML / 存在しない `master` ブランチ）と SUMMARY の旧フォーク手順（`cargo uninstall` / `git checkout feat/all-features`）を、今確実に動くソースビルド（`git clone` + `cargo install --path .`）に統一。※ `install.sh` は GitHub releases 依存のため、リリース・ペンディング中は `curl|sh` が失敗する点も考慮。→ [499c7ea]
- プラグインテストの実行確認: **Python hermes は 18/18 PASS**（維持対象）。シェル統合テスト2本は CI 未組込かつ旧契約（audit が native `bdo hook` へ移行 / Copilot の deny→自動 rewrite）を検証しており drift → **廃止**。
- `fix(hook)`: audit writer（`audit_log_inner`）が固定パスで `BDO_AUDIT_DIR` を無視していた writer/reader 不整合を解消。リーダーの `default_log_path()`（`pub(crate)` 化）を再利用。→ [17e960d]
- `refactor(hooks)`: リネーム漏れの完了 + 廃止。`hooks/{claude,cursor}/rtk-rewrite.sh`→`bdo-rewrite.sh`（`# rtk-hook-version:`→`# bdo-hook-version:` を `hook_check.rs` パーサ・テストと協調修正、`rtk-hook-version` 文字列ゼロ）。停滞シェルテスト `hooks/{claude,copilot}/test-bdo-rewrite.sh` を削除し、参照していた README の Testing 節も除去（stale `rtk-awareness.md`→`bdo-awareness.md` も修正）。→ [cc98821]

- `feat(read)` + `fix(read)`: `cat`/`head` の raw 取得性を改善。明示的 line window（`head -N`/`tail -N`/`--max-lines`）は **生 content に window**（フィルタをバイパス）して native head/tail 互換に。`filter::plain_head` で正確な先頭N行、`--max-lines 0` は空出力。縮約ビュー時は stdout flush 後に stderr へ raw 回復ヒント（`bdo read <file> -l none`）。→ [fc463b2] [6c29ace]
- `docs`: `hooks/copilot/README.md` の Copilot CLI 挙動を実態（`modifiedArgs` 透過 rewrite）に修正。README/README_ja に Python outline/map の機能例（`async def … : …`・`class …: …`）を追記。→ [f02ced5]
- `refactor(init)`: レガシー hook-script 移行サブシステムを撤去。`migrate_old_hook_script` / `remove_legacy_settings_entries` / `remove_legacy_hook_entries_from_json`（migrate からのみ到達）+ 専用テスト4件 + 呼び出し元2箇所（約246行）を削除。**script フックのライフサイクル（同梱 `bdo-rewrite.sh` / `integrity.rs` / `hook_check.rs` / install・uninstall 検知 `contains(REWRITE_HOOK_FILE)`）は手動インストール用に保持**（`hook_already_present` が script エントリを認識し二重登録を防止）。ついでに stale な `~/.config/rtk`→`bdo` コメントとステップ番号を修正。→ [b9e516c]

**実態メモ**: push は随時運用（同期状況は固定ハッシュを書かず `git rev-list --count origin/main..HEAD` で確認。この台帳更新コミット自体は push 前のため直後は local が 1 ahead）。git から導けない情報のみ記録 → **release バイナリ（installed `bdo`）は `6c29ace` までを `cargo install --path .` で反映済み**（`head`/`tail` 忠実化・`--max-lines 0`・audit `BDO_AUDIT_DIR`・`bdo-rewrite` リネーム等を実機確認）。`b9e516c`（init 移行サブシステム撤去）以降は init フロー refactor でランタイムのトークン挙動に影響しないが installed バイナリ未反映。Qiita 改善記事の下書きは `docs/bushido/qiita-improvements.md`（`.git/info/exclude` でローカル除外・非コミット）。

**残課題（リリースと独立）**
- （将来）`cat f | shasum` などパイプ時の raw passthrough（選択肢C）: エージェントは常にパイプ実行のためフィルタが広範に無効化される副作用があり、要設計判断。
- （任意）`b9e516c` を反映する release バイナリ再ビルド。

## 次の作業候補（リリースはペンディング）

1. レビュー・ドッグフーディング継続、必要な改善の実装。
2. 上記「残課題」の対応。
3. リリース方針が決まり次第: GitHub リリース成果物 / Homebrew tap 整備 → push / PR。

## 将来機能候補（バックログ）

ドッグフーディング観察（assistant）と外部 AI レビューの提案を統合。

### ✅ 実装済み（0.43.0 で消化）
`bdo review` / `bdo map --changed` / `bdo test --changed` / `bdo stale`（`.bdostaleignore` 対応）/ 検索系の欠落可視化（`find`/`grep` の `+N more`・`--all`）/ head・tail の raw 忠実化（`bdo read --raw-window` 相当）。

### ✅ 追加実装済み（0.43.0 で消化・第2弾）
- **変更系コマンドが `.bdostaleignore` を尊重**: `bdo review` の ARTIFACTS / STALE MARKERS 走査に `stale` と同じ ignore を適用。`load_stale_ignore` を `core::residue::load_ignore` に集約して共有。CHANGED 一覧は従来どおり全変更ファイルを表示（ignore は flag 抑制のみ）。テスト追加（`residue::load_ignore`）。
- **`bdo map` の一行空ボディ署名化**: `pub fn f() {}` / `impl T {}` など同一行で開閉する block（net brace delta 0）を `outline_braces` の collapse_all 経路で署名化。`pub fn f() { … }` として map に出るよう修正。テスト追加（`test_signatures_one_line_body_kept`）。

### ✅ 追加実装済み（0.44.0）
- **`bdo test --changed` の多言語化**: 変更セットを言語別にプランニングする `core::testplan` を新設。Rust→`cargo test -- <stems>`、Go→`go test <./pkg dirs>`（変更 `.go` の親パッケージ）、Python→`pytest <test files> [-k "<stems>"]`（テストは直接・ソースは `-k` 連動）、JS/TS→`package.json` から runner を判定し `vitest related --run` / `jest --findRelatedTests`。複数言語が混在すれば各々を順に実行し、最初の非ゼロ exit を返す。シェルに渡るパス/識別子は `shell_quote`/`quote_join` で個別クォートし、スペース入りパスでも引数が割れない。テスト追加（`testplan` ユニット9件 + 統合 `tests/test_changed_multilang.rs` 2件）。

### ✅ 追加実装済み（bdo ci）
- **`bdo ci`（統合ゲート）**: `bdo review`（変更サマリ・情報のみ）+ `bdo stale`（tree 全体の残骸ゲート）+ `bdo test --changed`（変更セットのテストゲート）を 1 コマンドで実行し**単一 exit code** を返す pre-merge ゲート。軽い検査（review, stale）を先に・遅いテストを最後に実行するが、**全ステージを常に走らせて**1 パスで全ブロッカーを提示（fail-fast しない）。exit code は「テスト失敗の実コード（例: cargo の 101）を残骸ゲートの 1 より優先」。`--against <ref>` は change-set 系（review / test --changed）に伝播、stale は常に tree 全体。実装は `cmds/system/ci.rs`（`automod` 自動登録）。`test --changed` のロジックを `ci::run_changed_tests` に切り出して main.rs の Test アームと共有（重複排除）。`is_operational_command` には非追加（review/stale と同じくフックパイプライン外のユーザー/CI 直実行）。純関数 `gate_exit_code` にユニット4件 + scratch git で 4 ケース（clean / 変更のみ / テスト失敗 101 / 残骸 1）を E2E 検証。全 2250 テスト green。

### 新規候補（assistant の欲しい機能・残）
- **`bdo stale` の docs↔impl コマンド名ズレ検出**: ドキュメント中の `bdo <cmd>` 参照のうち `bdo --help` に存在しないものを検出（元バックログの未実装分）。
- **`.bdostaleignore` の行内サプレッション**: ファイル glob に加え、`# bdo-stale-ignore` 行内マーカーで 1 行単位の除外（文書化された残骸の局所許可）。

### 地味だが効く
- **共通 raw バイパスの統一**: `-l none` / `BDO_NO_TOML` / passthrough が散在。全コマンド共通の `--raw` / `BDO_RAW=1` に集約し学習コスト減。
- **`BDO_QUIET=1`**: reduced-view ヒント等の stderr 補助メッセージを抑制。
- **`bdo map` シンボルフィルタ / ドリルダウン**: `bdo map --grep <sym>`、map から特定ファイルを outline へ。

### 要切り分け
- **複合コマンド/パイプ堅牢性**: `cmd && echo` 連鎖の出力切れ・exit code 干渉の事象あり。フィルタが stdout/exit code を変えない保証（ハーネス側要因の可能性もあり要調査）。
- **`cat f | shasum` 等パイプ時の raw passthrough**: エージェントは常にパイプ実行のためフィルタが広範に無効化される副作用があり要設計判断（選択肢C）。
