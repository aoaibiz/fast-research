# Contributing

rustxng への関心ありがとうございます。バグ報告・機能提案・プルリクエストを歓迎します。本プロジェクトは AGPL-3.0-or-later で公開されているため、コントリビューションも同じライセンスの下で取り込まれます。

## はじめる前に

1. 既存の [Issue](https://github.com/sahenjp/RustXNG/issues) と [docs/PORTING.md](docs/PORTING.md) を確認してください（移植方針と未対応範囲が書かれています）
2. 大きな変更は、先に Issue で方針を相談してください
3. 脆弱性を見つけた場合は Issue ではなく [SECURITY.md](SECURITY.md) の手順で報告してください

## 開発環境

- Rust 1.85 以上（エディション 2024）
- 追加のシステム依存はありません

```bash
git clone https://github.com/sahenjp/RustXNG.git
cd rustxng
cargo test
```

## 変更の種類と確認コマンド

| 変更 | 必須の確認 |
| :--- | :--- |
| すべての変更 | `cargo fmt --all --check`、`cargo test` |
| ロジック変更 | 上記 + `cargo clippy --all-targets -- -D warnings` |
| エンジン追加・修正 | 上記 + フィクスチャテストの追加（下記） |
| 性能に関わる変更 | 上記 + `cargo run --release --example benchmark` |

## エンジンを追加する

1 エンジン = 1 ファイルです。

1. `src/engines/<name>.rs` を作成し、`src/engines/duckduckgo.rs`（HTML）または `src/engines/wikipedia.rs`（JSON）を雛形にします
2. ファイル先頭に `//! <Name> engine, ported from \`searx/engines/<name>.py\`` と `// engine-id: <id>` を書きます
3. `Engine` トレイトの `meta` / `build_request` / `parse` を実装します（その他のアクセサは既定実装がメタデータを返します）
4. ネットワークを使わないフィクスチャテストを `#[cfg(test)] mod tests` に追加します
5. `src/engines/mod.rs` の `build()` と `settings.toml` に登録します

宣言的な定義で済む場合は、コードを書かずに上流 `settings.yml` の `xpath` / `json_engine` / `mediawiki` エントリを取り込めます（`tools/import_engines.py`）。

## コミットとプルリクエスト

- コミットメッセージは Conventional Commits（`feat:`、`fix:`、`docs:`、`refactor:`、`test:`、`ci:`）に従ってください
- 1 つの PR は 1 つの目的に絞ってください
- PR の説明には「概要 / 変更 / 検証 / 注意点」を書いてください
- テストが通らない PR はレビューしません（CI で同じ確認が走ります）

## リリース

`main` の CI が緑になってから、annotated タグを打つと `.github/workflows/release.yml` が走ります。

1. `Cargo.toml` の `version` と `CHANGELOG.md` に同じ版の項を用意します
2. `git tag -a vX.Y.Z -m 'vX.Y.Z'` して push します
3. CI が 5 ターゲットをビルドし、`rustxng-vX.Y.Z-<target>`（Windows は `.exe`）と `.sha256` を Release に添付します

リリース本文は `CHANGELOG.md` の該当版セクションから自動で作られます。手順そのものを直したあとに同じタグを出し直したい場合は、タグを動かさずに再実行できます（ワークフロー定義はディスパッチした ref から読まれます）。

```sh
gh workflow run release.yml --ref main -f tag=vX.Y.Z --dry-run=false
```

`--dry-run=true` にすると、ビルドと検証だけして何もアップロードしません。公開済みのアセットは上書きしません（未添付のものだけ追加します）。

## ライセンスと帰属

- 新しいファイルには SPDX ヘッダ（`// SPDX-License-Identifier: AGPL-3.0-or-later`）を推奨します
- 上流 SearXNG のコード（Python）をコピーしないでください。URL・パラメータ・抽出項目といった仕様情報を参照し、Rust で実装してください
- 上流に由来するデータを持ち込む場合は [NOTICE](NOTICE) に出典を追記してください

## リポジトリサイズについて

`bench/content-*.json`（商用 API の取得結果）と `index-data/`（自前インデックス）は**リポジトリに含めません**。再生成できるためです。

- 商用 API の取得結果: `tools/collect_commercial.py`（各サービスの API キーが必要）
- 自前インデックス: `rustxng-index crawl`（`docs/LOCAL-INDEX.md`）

過去のコミットには 28 MB の JSON が残っています（HEAD からは削除済み）。履歴を掃除する場合は、全タグの force-push でリリースビルドが再実行されるため、GitHub Actions を一時停止したうえで `git filter-repo` を実行してください。
