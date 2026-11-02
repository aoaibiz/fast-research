"""Measures the private (社内資料) search on a synthetic corpus: N documents of M characters each, through the real Store (FTS5 index).
Usage: python3 -m workbench.bench_private_search --docs 10000 --chars 30000 --out bench.json [--dir <folder>]
Reports: insert time, database size, and the search latency (p50 / max) for a handful of queries, with the first title each returns."""
import argparse
import json
import os
import random
import statistics
import tempfile
import time
from pathlib import Path

from .store import Store

KANA = 'アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワガギグゲゴザジズゼゾダヂヅデドバビブベボパピプペポー'
KANJI = '会員投稿予約時刻料金月額契約配信画面設定決済確認資料検索公開社内保存調査顧客決定記録反映関所試験本番画像動画文字番号期限通知更新削除登録一覧状態結果理由方法手順'


def vocabulary(rng, n=3000):
    words = []
    for _ in range(n):
        kind = rng.random()
        if kind < 0.45:
            words.append(''.join(rng.choice(KANJI) for _ in range(rng.randint(2, 4))))
        elif kind < 0.85:
            words.append(''.join(rng.choice(KANA) for _ in range(rng.randint(2, 6))))
        else:
            words.append(''.join(rng.choice('abcdefghijklmnopqrstuvwxyz') for _ in range(rng.randint(3, 8))))
    return words


def body_text(rng, words, chars):
    parts, size = [], 0
    while size < chars:
        sentence = ''.join(rng.choice(words) for _ in range(rng.randint(4, 12))) + rng.choice(['。', '、', '\n'])
        parts.append(sentence); size += len(sentence)
    return ''.join(parts)[:chars]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--docs', type=int, default=10000); ap.add_argument('--chars', type=int, default=30000)
    ap.add_argument('--out', default='bench-private-search.json'); ap.add_argument('--dir', default=None); ap.add_argument('--seed', type=int, default=7)
    ap.add_argument('--queries-only', action='store_true', help='reuse the corpus already in --dir (measure the queries again after a code change)')
    args = ap.parse_args()
    rng = random.Random(args.seed); words = vocabulary(rng)
    folder = Path(args.dir) if args.dir else Path(tempfile.mkdtemp(prefix='fast-research-bench-'))
    folder.mkdir(parents=True, exist_ok=True)
    # the bench measures search speed, so its caps are sized to the corpus (Japanese text is ~3 bytes a character)
    limits = {'document_chars': max(args.chars, 2_000_000), 'documents_per_owner': max(args.docs + 10, 10_000), 'document_bytes_per_owner': max(300_000_000, args.docs * (args.chars + 40) * 4)}
    store = Store(folder / 'bench.sqlite', limits)
    owner = store.first_admin()['id'] if args.queries_only else store.register('bench', 'bench-password-2026-xx')
    planted = ['ハルモニアの料金', 'Orion Suiteについて', '写真メイトの動画の長さ', '決済の通知の遅れ', 'Draftly 下書き']
    started = time.monotonic(); total_chars = 0
    for i in range(0 if args.queries_only else args.docs):
        body = body_text(rng, words, args.chars)
        title = f'資料 {i}'
        if i % 500 == 0:  # a few documents carry the planted phrases in the title (and body), like real imported pages
            phrase = planted[(i // 500) % len(planted)]
            title = f'{phrase} の説明 {i // 500}'; body = phrase + 'は次の通り。' + body
        store.add_document(owner, title, body); total_chars += len(body)
    insert_seconds = time.monotonic() - started
    size_bytes = os.path.getsize(folder / 'bench.sqlite')
    queries = planted + ['会員の料金', '予約した時刻に投稿', 'edge worker', '存在しない語句ゼロ件']
    results = {}
    for q in queries:
        times = []; first = None
        for _ in range(5):
            t0 = time.monotonic(); rows = store.search_documents(owner, q); times.append(time.monotonic() - t0)
            first = rows[0]['title'] if rows else None
        results[q] = {'p50_ms': round(statistics.median(times) * 1000, 1), 'max_ms': round(max(times) * 1000, 1), 'hits': len(rows), 'first_title': first}
    report = {'docs': args.docs, 'chars_per_doc': args.chars, 'total_chars': total_chars, 'fts5': store.fts, 'insert_seconds': round(insert_seconds, 1),
              'sqlite_bytes': size_bytes, 'list_ms': None, 'queries': results, 'folder': str(folder)}
    t0 = time.monotonic(); n = len(store.documents(owner)); report['list_ms'] = round((time.monotonic() - t0) * 1000, 1); report['listed'] = n
    Path(args.out).write_text(json.dumps(report, ensure_ascii=False, indent=1), encoding='utf-8')
    print(json.dumps(report, ensure_ascii=False, indent=1))


if __name__ == '__main__':
    main()
