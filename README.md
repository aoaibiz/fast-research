# 高速リサーチ (fast-research)

**自分の AI に「出典つきの Web 検索」を足す、無料の道具です。**

- MCP に対応した AI(Codex CLI・Claude Code・Claude Desktop・Cursor・Gemini CLI など)に入れると、AI が公開 Web を検索し、**出典 URL・本文の該当箇所・取得できたかどうか**を受け取れます。回答の文章は、あなたの AI が作ります。
- 検索 API の契約・キー・利用料は要りません。検索は**あなたの PC から**各検索サイトへ行います。当方のサーバーは通りません。
- 検索と本文の取得を 1 回でまとめて返すので、AI が自分でページを 1 枚ずつ開くより待ち時間が短く済みます(速さは回線・検索サイトの応答で変わります)。

[sahenjp/RustXNG](https://github.com/sahenjp/RustXNG)(SearXNG の Rust 移植)を基に、安全な本文取得・根拠の品質表示・AI 用の起動方式を足した派生版です。上流の commit `fef998b` から作り、AGPL-3.0-or-later・LICENSE・NOTICE をそのまま保持しています。上流の変更の履歴は上流の置き場にあります。

## 入れ方(Mac / Linux。Windows は WSL の中で)

必要な物: Git・Python 3.9 以上・Rust(`cargo`)・`lsof`。Rust が無ければ [rustup](https://rustup.rs/) で入れます。最初のビルドは PC によって数分〜十数分かかります(12 コアの PC で約 4 分)。

```sh
git clone https://github.com/aoaibiz/fast-research.git
cd fast-research
cargo build --locked --release --no-default-features --bin rustxng
```

続けて、使っている AI に登録します(同じフォルダーで実行。`$(pwd)` がこのフォルダーの絶対パスになります)。

```sh
# Codex CLI
codex mcp add fast-research -- python3 "$(pwd)/workbench/agent.py"

# Claude Code(全プロジェクトで使う場合)
claude mcp add --transport stdio --scope user fast-research -- python3 "$(pwd)/workbench/agent.py"
```

前の名前 `talmud-research` で入れた方は、そのままでも動きます(名前は表示だけ)。Claude Desktop・Cursor・Gemini CLI の書き方、外し方、設定の項目は [docs/INSTALL-AI.md](docs/INSTALL-AI.md) にあります。

**AI に入れてもらう場合**は、自分の AI(手元の PC で動く物)にこう頼みます。

> https://github.com/aoaibiz/fast-research の docs/INSTALL-AI.md を読み、この PC のあなた自身に MCP サーバー「fast-research」として入れてください。今ある設定は消さずに足してください。入れたら公開情報の検索を 1 回して、出典つきで答えられるか確かめてください。

## 確かめ方

AI を開き直してから、例えばこう聞きます。

- 「Python の pathlib の役割を、公式ドキュメントを根拠に短く説明して」
- 「〇〇(サービス名)の料金を公式の資料で調べて、出典つきで整理して」

AI が `search_public` という道具を使い、回答に出典 URL が付けば入っています。

## 自分の社内資料と一緒に使う時の考え方

```text
あなたの質問
  ├─ 社内資料 … あなたの AI が、あなたの PC の中で読む(この道具は資料を読まない・送らない)
  └─ 公開情報 … あなたの AI が「公開してよい検索語」だけを作り、この道具が検索サイトへ送る
→ あなたの AI が両方を合わせて答える
```

- この道具が外へ送るのは**検索語だけ**です。資料の中身・ファイル・会話は送りません。既定の入れ方では、AI 用の入口(`workbench/agent.py`)は資料のデータベースを開きません。
- 社内資料は、AI 自身のファイル読み取り機能や、あなたが入れた別の道具で読ませます。資料をどの AI 事業者に見せるかは、あなたと AI の契約の話で、この道具では変わりません。

## 手元の資料も AI から使う(任意・自分で選んだ時だけ)

登録の時に `--private` を付けた時だけ、AI が**この PC の画面に保存した資料と調査**を検索・読み(`search_private`・`read_private`)、大事な話の要点を資料として保存(`save_note`)できるようになります。付けなければ、今までどおり公開 Web の検索だけです。

```sh
codex mcp add fast-research -- python3 "$(pwd)/workbench/agent.py" --private
claude mcp add --transport stdio --scope user fast-research -- python3 "$(pwd)/workbench/agent.py" --private
```

- 資料はこの PC の `local-data/`(画面と同じデータベース)にあり、この道具は外へ送りません。ただし AI が読んだ資料の中身は、その AI の会話の一部として AI 事業者に届きます。
- 保存する時、鍵やトークンの形の文字列は消してから保存します。同じ題名と本文は二重に保存しません。1 回の検索・読み取りの返事は 4KB までです。
- AI への指示(カスタム指示・CLAUDE.md・AGENTS.md など)に、例えばこう書いておくと会話がたまっていきます。

> 大事な話の後は、要点を 高速リサーチ に保存する(fast-research の save_note・題名は内容が分かる短い文)。前に話した事を聞かれたら、先に search_private で探す。

- **ウェブ版の ChatGPT・claude.ai からは、この PC の道具に届きません**(この置き場に遠隔のサーバーはありません)。ウェブ版の会話は、要点を `.md` か `.txt` のファイルにして `local-data/inbox/` に置くと、画面を動かしている間 30 秒ごとに資料に入り、`local-data/inbox/done/` へ移ります。
- 手元の Codex・Claude Code の会話の記録を 1 日 1 件まとめて受け箱に入れる事もできます(任意): 画面を `--digest-ai-sessions` を付けて起動するか、`python3 -m workbench.digest --data local-data` を自分で(または毎日の予約で)動かします。読むのはこの PC の `~/.codex/sessions` と `~/.claude/projects` の、あなたが書いた文と AI の答えの文だけで、外へは送りません。詳しくは [docs/INSTALL-AI.md](docs/INSTALL-AI.md#手元の資料も使う任意)。
- 検索語にメールアドレスや鍵のような文字列が入っていると、送らずに止めます。社名・案件名など**外に出したくない語**は、1 行 1 語のファイルを作って環境変数 `FAST_RESEARCH_DENYLIST` に指定すると、その語を含む検索を止めます(設定は [docs/INSTALL-AI.md](docs/INSTALL-AI.md#設定できる事))。
- それでも、何を検索語にするかを最後に決めるのは AI です。AI への指示で「社内の固有名詞は検索語に入れない」と伝えておくと確実です。

## 限界と注意

- **検索サイトの規約**: 検索サイトの利用規約は各自で確認してください。この道具は Bing・Yahoo! JAPAN・DuckDuckGo・Brave の検索結果のページを自動で取得します。自動での取得を規約で制限しているサイトがあります。使う前に各サイトの規約を確認し、ご自身の責任で使ってください。多く使うと検索サイトに止められる事があります。この道具は止められた時に回避しません(CAPTCHA を解かない・止められた検索サイトはしばらく休ませる)。
- **名乗り(User-Agent)**: 既定では、元の RustXNG と同じくブラウザー(Firefox)の名乗りを回して使います。この道具だと名乗る(`TalmudResearch/0.26 (+https://github.com/aoaibiz/fast-research)` に固定する)には、環境変数 `FAST_RESEARCH_IDENTITY=honest` を設定します([設定できる事](docs/INSTALL-AI.md#設定できる事))。名乗りによって検索サイトの返す件数が変わります。2026-11-02 の実測では、Yahoo! JAPAN はこの道具の名乗りには結果の無いページを返し、ブラウザーの名乗りには通常の結果を返しました。
- **正しさ**: 取得できた本文の真偽・最新性・質問への完全な答えは保証しません。取得の失敗・本文に一致箇所が無い事は、そのまま「取得できなかった」と返します。
- **使える AI**: 手元の PC で MCP の道具(stdio)を動かせる AI だけです。**ウェブ版の AI(chatgpt.com・claude.ai など)からは、手元の PC の道具に直接つなげません**(インターネットに置いた「遠隔の MCP サーバー」が要り、この置き場の範囲外です)。
- **OS**: macOS と Linux。Windows では WSL2(Ubuntu など)の中で使います。Windows ネイティブは対象外です。
- 道具そのものは無料ですが、AI の利用料・PC の電気代などは別です。
- バックアップの復元は、画面が 8MB ずつ分けて送り、PC の中で 1 件ずつ読んで戻します。上限は戻した後の資料の容量(既定 300MB)で見ます。復元の間(資料 300MB で数十秒〜数分)、画面の他の保存は少し待たされる事があります。

## 画面で使う(任意)

AI を使わずに自分で調べる時や、出典を見比べたい時は、手元だけで動く画面もあります。

```sh
python3 -m workbench.server --binary "$(pwd)/target/release/rustxng"
```

ブラウザーで http://localhost:18890 を開きます。画面に登録した資料は、この PC の中(`local-data/`)だけに保存され、外へは送りません。資料は行のごみ箱から消せます。使い方は [docs/WORKBENCH.md](docs/WORKBENCH.md)。

## 資料

- [自分の AI に入れる(AI ごとの書き方・設定)](docs/INSTALL-AI.md)
- [AI に渡す使い方の決まり](docs/AI-USAGE.md)
- [画面の使い方・保存・戻し方](docs/WORKBENCH.md)
- [24 問の比較と限界](docs/LOCAL-EVALUATION.md)
- [上流 RustXNG の README(保存版)](docs/UPSTREAM-README.md) — 上流の性能の数字は、この派生版の実測ではありません。

## ライセンス

AGPL-3.0-or-later([LICENSE](LICENSE)・[NOTICE](NOTICE))。手元で使うだけなら公開の義務はありません。改変した物をネットワーク越しに他人へ使わせる場合は、その改変版のソースを利用者に提供する必要があります(画面の「ソースを取得(AGPL)」から ZIP を取得できます)。
