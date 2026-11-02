# rdsh から使う

rdsh は `search-web` のバックエンドとして RustXNG を直接使えます。**追加設定は不要**です。RustXNG の既定ポート（8888）が rdsh の探索先と一致しています。

## 使い方

```bash
# 1. RustXNG を起動（既定 8888）
rustxng

# 2. rdsh から検索
rdsh search-web --engine rustxng "python dict get"
rdsh search-web --engine rustxng --json "python dict get"
```

## 解決順

`rdsh search-web --engine auto`（既定）は次の順に探します。

1. `SEARXNG_URL`
2. `RUSTXNG_URL`
3. ローカルの `rustxng` コマンド
4. `http://127.0.0.1:8888`

RustXNG を別ホスト・別ポートで動かす場合:

```bash
RUSTXNG_URL=http://127.0.0.1:9999 rdsh search-web --engine rustxng "query"
```

## バイナリの導入

`rdsh rustxng install` は GitHub Releases の最新版を `~/.local/bin` へ入れます。RustXNG 側のリリースは 5 プラットフォーム（Linux gnu/musl、macOS aarch64/x86_64、Windows）にバイナリを添付しています。

## 互換性

rdsh は SearXNG の JSON API を読むため、RustXNG は同じ形で応答します。

- `/search?q=...&format=json` の `results[]`（`title` / `url` / `content`）
- `/healthz`（起動確認）
- `unresponsive_engines` は `[engine, 理由]` の配列

2026-10-31 時点で、`rdsh search-web --engine rustxng --json` が RustXNG の応答をそのまま解釈できることを実機で確認しています。

## rdsh と併用するときの推奨設定

- **自前インデックスを併用する** — `index-data/` を置いておけば、追加設定なしでローカルのヒットが上流の結果に足されます（hit@1 +4）
- **キャッシュ** — rdsh は同じクエリを繰り返しがちです。既定のキャッシュ（300 秒）で 2 回目以降は 16 ms で返ります
- **`--limit`** — rdsh 側の件数指定に合わせて `max_results` が渡ります

## サイドカーの後始末

rdsh は `search-web` のたびに RustXNG をサイドカーとして起動します。環境によってはそのプロセスが残り続けることがあるため、**アイドル終了**を用意しています。

```bash
# 5 分間リクエストが無ければ自動で終了する
RUSTXNG_IDLE_TIMEOUT=300 rustxng
```

`settings.toml` の `server.idle_timeout` でも指定できます（既定 0 = 無効）。
