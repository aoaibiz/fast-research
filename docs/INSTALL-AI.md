# 自分の AI に入れる

高速リサーチ(fast-research)は、MCP の「stdio」(手元の PC で道具を起動して標準入出力でつなぐ方式)に対応した AI なら、どれにでも入れられます。入れるのは 1 本のコマンドだけです。

```text
python3 /このフォルダーの絶対パス/workbench/agent.py
```

- 先に [README](../README.md) の「入れ方」で `cargo build --locked --release --no-default-features --bin rustxng` を済ませます。
- 絶対パスは、このフォルダーで `python3 workbench/agent.py --print-config` を実行すると表示されます(その PC 用の設定例が出るだけで、どの設定も書き換えません)。
- 道具は AI が起動した時だけ動き、AI を閉じると一緒に終わります。常駐のサービス・固定のポート・アカウントは要りません。
- 前の名前 `talmud-research` で入れた設定は、そのままでも動きます(名前は表示だけ)。環境変数も前の名前(`TALMUD_RESEARCH_…`)をそのまま読みます。
- 下の書き方は、各 AI の公式の資料で **2026-11-02** に確認した物です。AI 側の更新で変わる事があるので、うまくいかない時は各 AI の公式の資料を見てください。

## Codex CLI

公式: https://learn.chatgpt.com/docs/extend/mcp?surface=cli(確認 2026-11-02・2026-11-02 に実際に入れて検索 1 回を確認)

```sh
codex mcp add fast-research -- python3 "/絶対パス/workbench/agent.py"
codex mcp list
```

設定ファイル(`~/.codex/config.toml`)に直接書く場合:

```toml
[mcp_servers.fast-research]
command = "python3"
args = ["/絶対パス/workbench/agent.py"]
tool_timeout_sec = 120
```

- 公式の資料によると、ChatGPT のデスクトップアプリ・Codex CLI・IDE 拡張はこの設定を共有します。
- 1 回の検索は本文の取得を含めて数秒〜数十秒かかる事があります。既定の道具の待ち時間は 60 秒なので、`tool_timeout_sec = 120` を勧めます。
- 外す: `codex mcp remove fast-research`

## Claude Code

公式: https://code.claude.com/docs/en/mcp(確認 2026-11-02・2026-11-02 に実際に入れて検索 1 回を確認)

```sh
claude mcp add --transport stdio --scope user fast-research -- python3 "/絶対パス/workbench/agent.py"
claude mcp list
```

- `--scope user` は全プロジェクトで使う設定です。省くと今のプロジェクトだけ(`local`)になります。`--scope project` はプロジェクトの `.mcp.json` に書かれ、そのフォルダーを共有する人にも見えます。
- 環境変数を渡す時は `--env KEY=value` を `--transport stdio` より前に置きます(例は下の「設定できる事」)。
- Claude Code の中では `/mcp` で接続の状態を見られます。
- 外す: `claude mcp remove fast-research`

## Claude Desktop(macOS・Linux)

公式: https://modelcontextprotocol.io/docs/develop/connect-local-servers(確認 2026-11-02・実機での動作は未確認)

設定 → Developer → Edit Config で開く `claude_desktop_config.json`(macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`、Linux: `~/.config/Claude/claude_desktop_config.json`)の `mcpServers` に足します。今ある項目は消さずに足してください。

```json
{
  "mcpServers": {
    "fast-research": {
      "command": "python3",
      "args": ["/絶対パス/workbench/agent.py"]
    }
  }
}
```

保存したら Claude Desktop を完全に終了して開き直します。`python3` が見つからないと言われる時は、`which python3` で出る絶対パスを `command` に書きます。Windows 版の Claude Desktop から WSL の中の道具を動かす方法は、確かめていないので載せていません。

## Cursor

公式: https://cursor.com/docs/context/mcp(確認 2026-11-02・実機での動作は未確認)

全プロジェクトで使う場合は `~/.cursor/mcp.json`、そのプロジェクトだけなら `.cursor/mcp.json` に書きます。

```json
{
  "mcpServers": {
    "fast-research": {
      "type": "stdio",
      "command": "python3",
      "args": ["/絶対パス/workbench/agent.py"]
    }
  }
}
```

## Gemini CLI

公式: https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md(確認 2026-11-02・実機での動作は未確認)

```sh
gemini mcp add -s user fast-research python3 "/絶対パス/workbench/agent.py"
```

- `-s user` を省くと、今のプロジェクト(`.gemini/settings.json`)だけの設定になります。
- 設定ファイル(`~/.gemini/settings.json`)に直接書く場合は、`mcpServers` に `"fast-research": {"command": "python3", "args": ["/絶対パス/workbench/agent.py"]}` を足します。

## ウェブ版の AI(chatgpt.com・claude.ai など)

手元の PC の道具には直接つなげません。ウェブ版の AI がつなげるのは、インターネットに置いた「遠隔の MCP サーバー」だけです(OpenAI の資料「ChatGPT web doesn't read local Codex configuration files」・MCP の資料「Connect to remote MCP Servers」)。この道具を遠隔の MCP サーバーとして公開する作りは、この置き場には入っていません。ウェブ版の会話を手元の資料に入れたい時は、要点を `.md` か `.txt` のファイルにして画面のデータの `inbox/` に置きます(下の「手元の資料も使う」)。

## 手元の資料も使う(任意)

登録のコマンドの最後に `--private` を足した時だけ、AI に次の 3 つの道具が加わります。足さなければ公開 Web の検索(`search_public`)だけです。

| 道具 | すること |
|---|---|
| `search_private` | 画面に保存した資料と調査を検索し、題名・抜粋・id を返す(1 回 4KB まで・画面の社内資料の検索と同じ並び順) |
| `read_private` | id の本文を `offset` の文字から 1 回 4KB まで返す(`next_offset` があれば続きがある) |
| `save_note` | 題名と本文(2 万字まで)を資料として保存する。鍵・トークンの形の文字列は消してから保存し、同じ題名と本文は二重に保存しない |

```sh
codex mcp add fast-research -- python3 "/絶対パス/workbench/agent.py" --private
claude mcp add --transport stdio --scope user fast-research -- python3 "/絶対パス/workbench/agent.py" --private
```

- JSON や toml の設定では `args` に `"--private"` を足します。画面のデータを別の場所に置いている時は `"--data", "/そのフォルダー"` も足します(既定はこのフォルダーの `local-data/`=画面と同じ)。
- 資料はこの PC の中にあり、この道具は外へ送りません。ただし、AI が読んだ資料の中身は、その AI の会話の一部として AI 事業者に届きます。読ませたくない資料は画面から消してください。
- AI への指示(Codex の AGENTS.md・Claude Code の CLAUDE.md・各 AI のカスタム指示)にこう書くと、大事な話がたまっていきます:

> 大事な話の後は、要点を 高速リサーチ に保存する(fast-research の save_note・題名は内容が分かる短い文)。前に話した事を聞かれたら、先に search_private で探す。

### ウェブ版の AI の会話・自分のメモ(受け箱)

ウェブ版の ChatGPT・claude.ai は手元の道具を呼べません。会話の要点を `.md` か `.txt` のファイルにして、画面のデータの `inbox/`(既定 `local-data/inbox/`)に置きます。画面を動かしている間、30 秒ごとに資料に入り、`inbox/done/` へ移ります。鍵の形の文字列は消し、同じ中身は 1 回だけ入れます。同じ名前のファイルを中身を変えて置き直すと、前に入れた資料を差し替えます。

### 手元の AI の会話を毎日まとめる(任意)

この PC の Codex(`~/.codex/sessions`)と Claude Code(`~/.claude/projects`)の会話の記録から、あなたが書いた文と AI の答えの文だけ(道具の出力は入れない)を 1 日 1 件の資料にします。外へは送らず、AI も呼びません。

- 画面と一緒に毎日: `python3 -m workbench.server --binary ... --digest-ai-sessions`(朝 3 時を過ぎたら前の日の分を 1 回)
- 自分で・予約で: `python3 -m workbench.digest --data local-data`(既定は前の日。`--day 2026-11-01` で日を選ぶ)。例(cron): `30 3 * * * cd /絶対パス && python3 -m workbench.digest --data local-data`

## 設定できる事

どれも環境変数で、AI の MCP 設定の中で道具に渡します(Codex / Claude Code は `--env KEY=value`、Gemini CLI は `-e KEY=value`、JSON の設定は `"env": {"KEY": "value"}`、Codex の toml は `[mcp_servers.fast-research.env]`)。

| 環境変数 | 既定 | 中身 |
|---|---|---|
| `FAST_RESEARCH_IDENTITY`(前の名前 `TALMUD_RESEARCH_IDENTITY` も読む) | `browser` | 検索サイトと取得するページへの名乗り。`browser` = 元の RustXNG と同じ、ブラウザー(Firefox)の名乗りを回して使う。`honest` = `TalmudResearch/0.26 (+https://github.com/aoaibiz/fast-research)` の 1 つに固定(検索サイトによって結果が減る・README の「限界と注意」) |
| `FAST_RESEARCH_USER_AGENT` | (無し) | `honest` の時の名乗りの文字列を自分の物に替える(例: 連絡先の URL を入れる)。256 文字まで |
| `FAST_RESEARCH_DENYLIST` | (無し) | 外に出したくない語のファイル(1 行 1 語・2 文字以上・`#` で始まる行はコメント)。その語を含む検索は送らずに止める。指定したファイルが読めない時は、公開検索を全部止める |
| `FAST_RESEARCH_OFFICIAL_DOMAINS` | (無し) | 「公式」と表示するドメインを足すファイル(1 行 1 ドメイン)。同梱の `workbench/official-domains.txt` に足される |

例(Claude Code・この道具だと名乗る):

```sh
claude mcp add --env FAST_RESEARCH_IDENTITY=honest --transport stdio --scope user fast-research -- python3 "/絶対パス/workbench/agent.py"
```

## うまく動かない時

道具は失敗した理由を `stage` と `code` で返します。AI にそのまま見せれば、AI が直し方を判断できます。

| code | よくある理由 |
|---|---|
| `backend_missing` | `cargo build ... --bin rustxng` がまだ・別のフォルダーでビルドした |
| `backend_not_executable` / `backend_wrong_format` | 実行の権限が無い・別の OS や CPU 向けにビルドした |
| `backend_launch_failed` / `backend_listen_timeout` / `backend_exited_<数>` | 起動に失敗・`lsof` が無い(Linux は `sudo apt install lsof` など) |
| `search_unavailable` | 検索の待ち受けを確かめられない・回線が切れている |
| `possible_private_query` / `private_term_denied` | 検索語に鍵・メールの形、または `FAST_RESEARCH_DENYLIST` の語が入っている(わざと止めている) |
| `denylist_unreadable` | `FAST_RESEARCH_DENYLIST` に指定したファイルが無い・読めない・256KB を超える(公開検索を全部止めている)。ファイルを直すか、環境変数を外して AI を開き直す |
| `identity_invalid` | `FAST_RESEARCH_IDENTITY` が `honest` / `browser` 以外・名乗りの文字列が長すぎる |
