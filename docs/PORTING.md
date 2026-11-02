# SearXNG → Rust 移植メモ

このドキュメントは、SearXNG（<https://github.com/searxng/searxng>、AGPL-3.0-or-later）のどの部分を Rust に移植したか、どこで上流と意図的に違うかを記録したものです。上流のファイル名をそのまま引用しているので、差分を追うときはこの表を起点にしてください。

## 移植方針

1. **振る舞いを引き継ぐ**。クエリ構文、結果の統合・並び替え、設定の形式、API のパラメータ名とレスポンス項目は上流に合わせています。
2. **Python のコードはコピーしない**。エンジンの URL・パラメータ・抽出項目といった「事実」だけを参照し、実装は Rust で書き直しています。データとして移植したのはロケール表（`searx/sxng_locales.py`）と外部 bang の縮小版のみです。
3. **API キー不要のエンジンを優先する**。上流の 253 エンジンモジュールのうち、無料・無認証で動くものを選びました。
4. **違反・逸脱は隠さない**。上流と違う点はこのファイルに明記します。

## モジュール対応

| このリポジトリ | 上流 |
| --- | --- |
| `src/settings.rs`, `settings.toml` | `searx/settings_defaults.py`, `searx/settings_loader.py`, `searx/settings.yml` |
| `src/query.rs` | `searx/query.py` |
| `src/results.rs` | `searx/results.py`, `searx/result_types/` |
| `src/engine/mod.rs` | `searx/engines/__init__.py` |
| `src/engine/state.rs` | `searx/engines/__init__.py` の suspend / error 処理 |
| `src/engine/util.rs` | `searx/utils.py`, `searx/engines/xpath.py` 相当の補助 |
| `src/engines/*.rs` | `searx/engines/*.py` |
| `src/network.rs` | `searx/network/` |
| `src/cache.rs` | `searx/cache.py` |
| `src/metrics.rs` | `searx/metrics/` |
| `src/aggregator.rs` | `searx/search/__init__.py`, `searx/search/processors/online.py` |
| `src/plugins/*.rs` | `searx/plugins/*.py` |
| `src/autocomplete.rs` | `searx/autocomplete.py` |
| `src/locales.rs` | `searx/sxng_locales.py`（生成データ） |
| `src/external_bangs.rs` | `searx/external_bang.py`, `searx/data/external_bangs.json` |
| `src/web/mod.rs`, `src/web/handlers.rs`, `src/web/feeds.rs` | `searx/webapp.py`, `searx/webutils.py` |
| `templates/*.html`, `static/style.css` | `searx/templates/simple/*` |

## 機能ごとの対応状況

凡例: **移植済み** = 上流と同じ目的で動作 / **部分** = 動くが範囲や精度が狭い / **未移植** = 今回の対象外。

| 機能 | 状態 | 補足 |
| --- | --- | --- |
| クエリ構文（bang / category / language / timeout / feeling lucky） | 移植済み | `src/query.rs`。`!engine`、`!category`、`:lang`、`<3`、`!!`、`!!bang` |
| 外部 bang | 部分 | 上流は圧縮トライ（約 13,000 件）。本実装は主要 40 件の表 |
| 言語・ロケール | 移植済み | `searx/sxng_locales.py` を等価な表として移植（110 ロケール） |
| 結果の重複排除・スコア・グループ化 | 移植済み | `searx/results.py` の `calculate_score` / `get_ordered_results` と同じ式 |
| エンジンフレームワーク（paging / time_range / safesearch / language / weight / timeout） | 移植済み | `Engine` トレイト |
| エンジンのサスペンドと統計 | 移植済み | `ban_time_on_fail`、`max_ban_time_on_fail`、`suspended_times` を反映 |
| HTTP クライアント（UA ローテーション、プロキシ、リトライ、タイムアウト） | 移植済み | `outgoing` 設定に対応 |
| 時間範囲フィルタ（`time_range`） | 移植済み | UI のセレクタと API の両方から指定可能。エンジン側はネイティブな日付フィルタへ変換（Google News `when:`、Bing `qft`、Crossref `from-pub-date`、GitHub `pushed:>`、arXiv の日付順ソート、Stack Exchange `fromdate`、Hacker News `numericFilters`、YouTube / Dailymotion の期間指定） |
| 応答キャッシュ | 部分 | メモリ内 TTL + LRU のみ。上流の valkey / redis / sqlite バックエンドは未移植 |
| メトリクス | 移植済み | メモリ内カウンタ、`/stats` と `/metrics`（OpenMetrics / Prometheus）。エンジン指標は上流と同じ `searxng_engines_*` 名 |
| レート制限・bot 検出（`limiter.toml`、botdetection） | 部分 | `server.limiter = true` で動く簡易レート制限（クライアント単位の固定窓、`limiter_window` / `limiter_max`）。上流の botdetection（link token、Redis カウンタ、IP 範囲判定）は未移植 |
| 検索 API（html / json / csv / rss） | 移植済み | `format=` で切り替え |
| `/autocompleter` | 移植済み | duckduckgo / google / wikipedia / mwmbl |
| `/config`、`/preferences`、`/stats`、`/healthz`、`/opensearch.xml` | 移植済み | 設定は Cookie 保存（上流と同じ発想） |
| HTML UI（検索フォーム、結果一覧、ページ送り、設定画面） | 部分 | simple テーマ相当の 1 テーマのみ。テーマ切り替えは Cookie に保存するだけ |
| 多言語 UI（babel 翻訳） | 未移植 | UI 文言は英語のみ |
| プラグイン | 部分 | 7 件を移植（下表）。上流は 13 件 |
| アンサー（answerers: 通貨、天気、単位変換など） | 未移植 | `searx/answerers/` は対象外 |
| インフォボックス | 部分 | 型と描画は実装済み。生成するエンジンは未実装 |
| 結果タイプ（image / video / paper / file などの型付き結果） | 部分 | `SearchResult` + `metadata` で表現。型付きサブクラスは未移植 |
| 画像プロキシ（`image_proxy`） | 移植済み | HMAC-SHA256 署名（`server.secret_key`）と `/image_proxy?url=&h=`。上流に加えて私設アドレス・非 http(s) を拒否する SSRF ガードつき |
| ファビコン収集（`searx/favicons`） | 未移植 | |
| ベンチマーク（上流には無い） | 追加 | `examples/benchmark.rs` と `bench/compare.py`。[docs/BENCHMARK.md](BENCHMARK.md) |

### プラグイン対応

| 上流 | 状態 |
| --- | --- |
| `hostnames` | 移植済み（ホスト名の metadata 付与） |
| `tracker_url_remover` | 移植済み（内蔵のトラッキングパラメータ表。上流は ClearURLs ルールを取得） |
| `calculator` | 移植済み（四則演算・べき乗・sqrt などの簡易評価器） |
| `hash_plugin` | 移植済み（MD5 / SHA-1 / SHA-256 / SHA-384 / SHA-512 / CRC-32） |
| `time_zone` | 部分（都市名と固定オフセットの表。上流は地理 API を使用） |
| `oa_doi_rewrite` | 移植済み（既定無効） |
| `self_info` | 移植済み（既定無効） |
| `infinite_scroll` | 未移植（UI のみの機能） |
| `ahmia_filter` / `tor_check` | 未移植（Tor 前提のため対象外） |
| `unit_converter` | 未移植 |

## エンジン対応

| エンジン | 上流 | 逸脱・補足 |
| --- | --- | --- |
| duckduckgo | `duckduckgo.py` | HTML エンドポイント。上流の vqd 取得を伴う 2 ページ目以降の処理は未移植（`s@ オフセットで代替） |
| google | `google.py` | HTML。JS 必須のインタースティシャルを検出したら `access denied` として報告 |
| bing | `bing.py` | HTML。`/ck/a` の base64 リダイレクトを展開 |
| brave | `brave.py` | HTML + Cookie（safesearch / country / ui_lang） |
| mojeek | `mojeek.py` | HTML。上流の Altcha proof-of-work  solver は未移植で、チャレンジページは `captcha` として報告 |
| mwmbl | `mwmbl.py` | JSON API |
| google news | `google_news.py` | 上流は HTML をスクレイプ。本実装は同サービスの RSS を使用（JS 不要で公開日時と配信元が取れる） |
| bing news | `bing_news.py` | 上流は内部 JSON（`/news/infinitescrollajax`）。本実装は RSS を使用し、`qft` の期間指定は上流と同じ |
| wikipedia | `wikipedia.py` | MediaWiki API。上流の REST サマリ取得（インフォボックス）は未移植 |
| wikidata | `wikidata.py` | `wbsearchentities` のみ |
| openlibrary | `openlibrary.py` | `search.json` |
| hackernews | `hackernews.py` | Algolia API。上流と同じ Algolia チューニング（`restrictSearchableAttributes` など）と HN のコメント URL を既定に使用。`link_to_item = false` で記事 URL に切り替え可能 |
| github | `github.py` | `api.github.com`。`api_key` があれば Authorization を付与 |
| stackexchange | `stackexchange.py` | `api_site` でサイトを選択（既定 stackoverflow） |
| crates.io | `crates.py` | `/api/v1/crates` |
| npm | `npm.py` | 上流は npms.io。本実装は npm レジストリの検索 API（維持されている方）。索引が違うため結果セットは一致しない（[docs/BENCHMARK.md](BENCHMARK.md) 参照） |
| pypi | `pypi.py` | HTML 検索結果 |
| docker hub | `docker_hub.py` | `/api/search/v3/catalog/search` |
| arxiv | `arxiv.py` | Atom XML を quick-xml で解析。時間範囲指定時は `sortBy=submittedDate`（日付フィルタではなく並び順） |
| 時間範囲の扱い | — | `time_range` 指定時: crossref=`from-pub-date` + 新しい順、github=`sort=updated` + `pushed:>日付`、stackexchange=`sort=creation` + `fromdate`、hackernews=`numericFilters` |
| crossref | `crossref.py` | `/works`。JATS アブストラクトはタグ除去のみ |
| semantic scholar | `semantic_scholar.py` | 上流は内部 POST API（`X-S2-UI-Version` 必須）。本実装はキー不要の Graph API |
| openstreetmap | `openstreetmap.py` | Nominatim。wikidata 連携や経路アンサーは未移植 |
| photon | `photon.py` | `/api/` の GeoJSON |
| youtube | `youtube_noapi.py` | 継続トークンの POST は未移植のため 2 ページ目以降は保証しない |
| dailymotion | `dailymotion.py` | REST API。説明文は取得しない |
| deezer | `deezer.py` | 1 ページ 10 件（上流は 25 件） |
| openverse | `openverse.py` | 1 ページ 10 件（上流は 20 件） |
| wikicommons | `wikicommons.py` | MediaWiki API。`gsrprop=snippet` は使わず `extmetadata` から説明・作者・ライセンスを取得 |

### 意図的に対象外としたエンジン

- **API キーが必要なもの**（Brave API、Google CSE、Kagi、Exa、Wolfram など）
- **Tor 前提のもの**（Ahmia など）
- **海賊版・ファイル共有を目的とするもの**（torrent 系）。ライセンスではなく適法性の判断として外しています。
- **XPath ベースの汎用エンジン**（`settings.yml` の `engine: xpath`）。YAML で宣言された抽出規則を汎用に扱う仕組みは持たず、必要なものは個別モジュールとして実装しています。

## 既知の制約

- **IP レピュテーション**: DuckDuckGo は CAPTCHA、Google は JS インタースティシャルを返すことがあります。本実装はこれをエラーとして表示するので、結果 0 件と区別できます。
- **YouTube / Google の DOM**: セレクタは変わりやすいため、結果 0 件になる可能性があります。
- **上流のテスト**: 上流には 1 万件規模のテストがありますが、本移植のテストは独自のもの（ユニット 95 件 + 統合 9 件）です。
- **パフォーマンス**: 上流のプロセッサ分離（`online.py` のスレッドプール）に相当する仕組みは `tokio` の非同期タスクで代替しています。

## ライセンス上の配慮

- 本リポジトリは上流と同じ **AGPL-3.0-or-later**（[LICENSE](../LICENSE)）です。派生作品である以上、互換な条件を選ぶのが妥当と判断しました。
- 帰属は [NOTICE](../NOTICE) に、参照した上流ファイルと移植範囲を明記しています。
- データとして流用したのはロケール表と外部 bang（縮小版）のみで、いずれも上流と同じライセンスの下にあります。
- 上流のコード（Python）はコピーしていません。エンジンの URL・パラメータ・レスポンス項目は仕様情報として参照し、Rust で再実装しました。

## 今後の課題

1. 上流の Altcha solver 相当の実装（Mojeek など）と、botdetection 本体（link token、Redis カウンタ、IP 範囲判定）
2. YouTube の継続トークン対応、Google の JS 必須ページへの代替手段
3. favicon 収集（画像プロキシは実装済み）
4. valkey / redis バックエンドのキャッシュ（メモリ内キャッシュと Prometheus 出力は実装済み）
5. アンサー（通貨・天気・単位変換）とインフォボックスの生成側
6. UI の多言語化（上流の翻訳カタログの取り込み。現在は英語 UI のみ）
7. 上流テストスイートの取り込み（現在は独自テスト 120 件）
