# Ollama Web Search との比較

Ollama の Web Search API と RustXNG を、AI エージェントの検索ツールという観点で比べました。Ollama 側は公式ドキュメントの仕様、RustXNG 側は実機の実測値です。

## 結論

- **手軽さ・トークン効率・ページ取得まで一括**なら Ollama Web Search
- **無制限・自前データ・プライバシー・低レイテンシの大量検索**なら RustXNG
- 併用も現実的です。Ollama のエージェントに、Web 検索ツールとして RustXNG を差し込めます

## 仕様の比較

| 項目 | Ollama Web Search | RustXNG |
| --- | --- | --- |
| 形態 | マネージド API（ollama.com） | セルフホストの単一バイナリ |
| エンドポイント | `POST https://ollama.com/api/web_search` | `GET /search?q=...&format=json` |
| 認証 | `OLLAMA_API_KEY` 必須（無料アカウント） | 不要 |
| 料金 | 無料枠あり + 従量課金プラン | 無料（自前インフラのみ） |
| 検索対象 | 非公開（独自の検索基盤） | 169 エンジン（Google / Bing / DuckDuckGo / Wikipedia / GitHub / arXiv など） |
| 1 回の返却件数 | 1〜10 件（`max_results`、既定 5） | 数十〜数百件（例: 212 件 / 110 KB） |
| 絞り込み | `max_results` のみ | `time_range` / `categories` / `engines` / `!site` / `language` / `pageno` / `safesearch` |
| 返却フィールド | `title` / `url` / `content` | 上流互換 JSON（`score` / `engines` / `category` / `publishedDate` / `thumbnail` / `answers` / `infoboxes` など） |
| ページ取得 | `web_fetch` あり（本文 + リンク一覧） | なし（画像プロキシのみ） |
| キャッシュ | 非公開 | メモリ内 TTL + LRU / Valkey 共有 |
| 障害時の扱い | 非公開 | 応答しないエンジンを自動サスペンドし `unresponsive_engines` に記録 |
| SDK | Python / JavaScript（`web_search` / `web_fetch`） | REST のみ（MCP は薄いラッパーを自作） |
| ライセンス | サービス利用 | AGPL-3.0-or-later |

## レイテンシの実測

| 項目 | 値 | 備考 |
| --- | --- | --- |
| Ollama API への往復（認証エラー応答） | 0.21〜0.70 秒 | 検索処理は含まない下限値 |
| Ollama の検索レイテンシ | **未測定** | API キーが無いため実測できませんでした |
| RustXNG 初回（8 クエリ、生きているエンジンのみ待つ） | 中央値 3.00 秒 | 応答しないエンジンが `timeout_limit` まで待つため |
| RustXNG 2 巡目（サスペンド + キャッシュ後） | 中央値 **0.70 秒** | 以降は落ちたエンジンをスキップ |
| RustXNG 同一クエリの再実行（キャッシュヒット） | **1 ms 前後** | メモリ / Valkey キャッシュ |
| RustXNG 起動 / 常駐メモリ | 0.03 秒 / 13 MiB | 単一バイナリ |

## トークン効率

エージェントに渡す量は大きな差になります。

- Ollama: 1 回 10 件まで。スニペット付きでも数 KB 程度で、そのままプロンプトに入れられます（公式サンプルも 8,000 文字で切り詰めています）
- RustXNG: 一般クエリで中央値 **66 KB**（最大 110 KB）。そのままではコンテキストを圧迫するため、次のどれかが必要です
  - `engines=hackernews,arxiv` のように対象を絞る
  - `categories=news` や `time_range=day` で絞る
  - クライアント側で上位 N 件に切り詰める

## 使い分けの目安

| やりたいこと | 向いている方 |
| --- | --- |
| すぐ試したい / 自前インフラを持ちたくない | Ollama |
| トークン効率を最優先にしたい | Ollama |
| 検索してからページ本文も読みたい | Ollama（`web_fetch` 込み） |
| クエリを外部に出したくない | RustXNG |
| 回数制限を気にせず大量に検索したい | RustXNG |
| 社内 DB や独自ソースも同じ API で検索したい | RustXNG（PostgreSQL / MySQL / MongoDB / SQLite など） |
| サイト指定・新着指定・カテゴリ指定を細かく使いたい | RustXNG |

## 併用パターン

Ollama のエージェントは、ツールを差し替えられます。`web_search` の代わりに RustXNG を呼ぶ関数を渡せば、検索基盤だけを自前ホストに置けます。

```python
import urllib.parse, urllib.request, json

def rustxng_search(query, engines=None, time_range=None, limit=5):
    params = {"q": query, "format": "json"}
    if engines:
        params["engines"] = engines
    if time_range:
        params["time_range"] = time_range
    url = "http://127.0.0.1:8888/search?" + urllib.parse.urlencode(params)
    with urllib.request.urlopen(url, timeout=30) as response:
        data = json.load(response)
    return [
        {"title": item["title"], "url": item["url"], "content": item.get("content", "")}
        for item in data["results"][:limit]
    ]
```

戻り値を Ollama の `results` と同じ 3 フィールドに揃えておけば、プロンプト側はそのまま使えます。

## 参考

- Ollama Web search: <https://docs.ollama.com/capabilities/web-search>
- Ollama Cloud / API キー: <https://docs.ollama.com/cloud>
- RustXNG の速度: [BENCHMARK.md](BENCHMARK.md)
- RustXNG の精度: [ACCURACY.md](ACCURACY.md)
