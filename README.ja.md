# Sippion

[![Release](https://img.shields.io/github/v/release/Sitten-Tokyo/Sippion?label=release)](https://github.com/Sitten-Tokyo/Sippion/releases)
[![CI](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml/badge.svg)](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.Sitten--Tokyo%2Fsippion-blue)](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)](THIRD_PARTY_NOTICES.md)
[![Rust](https://img.shields.io/badge/rust-1.85-orange)](rust-toolchain.toml)

[English](README.md) | **日本語**

**聞くべき場所を先に絞る。読むのは3ファイルでいい。**

Sippion は、ローカル・読み取り専用MCPサーバーです。AIがファイルを開く前に、
リポジトリの必要最小限だけを渡します。公開ツールは `repo_context` の1つ。
関連コード断片と構造的な根拠を上限付きで返し、常時有効の小さな効率化ルールが
不要な質問・抽象化・説明を抑えます。

公開するMCPツールは `repo_context` の1つだけです。字句検索、構造情報、ソースコードだけを
対象にした意味ランキングを組み合わせ、関連度の高い小さなコード断片を返します。
`sippion setup` は対応クライアントへ、常時有効の小さなefficiency rule（効率化ルール）も設定します。
別モードや実行時のupstream prompt取得はありません。

## まずはインストール

1コマンドです。GitHubアカウントも追加ツールも不要。実行前にchecksumを検証します。

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/a28b611f169a2731ca89dd59db89ccf00940185f/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/a28b611f169a2731ca89dd59db89ccf00940185f/scripts/bootstrap.ps1 | iex
```

インストール後はこうなります。

```text
Sippionをインストール
    ↓
Codex + Claude Code + Antigravity + OpenCode に事前登録
    ↓
各AIクライアントを再起動
```

サプライチェーンの出所保証が必要な場合は、`SIPPION_STRICT_PROVENANCE=1` 付き
（sh）／ `$env:SIPPION_STRICT_PROVENANCE="1"` 設定後（PowerShell）に実行すると、
GitHub Artifact Attestationを実行前に検証します。詳細は
[Security and trust boundary](docs/security.md) を参照してください。

Sippionは、**4クライアントすべてに事前登録**します。今そのクライアントが
インストールされていなくても設定は作られます。

各クライアントはSippionを `--root-auto` で起動します。最も近いGit/project境界を
ルートに選び、home directoryやfilesystem rootを自動選択することはありません。
リポジトリごとに登録し直す必要はありません。境界ルールの詳細は
[Security and trust boundary](docs/security.md) を参照してください。

## 60秒で試す

初見のリポジトリで、エージェントにこう聞いてみてください。
「auth tokenの検証はどこ？」

Sippionなしでは、エージェントはリポジトリを総なめにして十数ファイルを開き、
回答前にコンテキストを使い切ります。Sippionありでは、先に1つだけ聞きます。

```text
repo_context {"q":"authentication token validation"}
```

返るのは数件の上限付き断片（例: `src/auth/validate.rs:42-89`、
`src/middleware/session.rs:12-40`）だけ。2ファイルを開いて回答します。
先に絞る、あとで読む。それが製品のすべてです。

|  | Grep / Glob | 常駐インデックス型 | Sippion |
|---|---|---|---|
| 導入 | なし | デーモン＋ディスク索引 | 1コマンド |
| 新鮮さ | 常に最新 | 再索引ラグあり | 常に最新（RAMのみ） |
| AIが見るもの | 素の一致 | リポジトリ丸投げの危険 | 上限付き断片 |
| 提供中の通信 | n/a | 場合による | なし |
| リポジトリへの書込 | なし | ある場合あり | なし |

## Official MCP Registry

SippionはOfficial MCP Registryに `io.github.Sitten-Tokyo/sippion` という名前で公開しています。
Registryの最新レコードは
[stable APIのSippion latest entry](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
から確認できます。

Registry配布対象の各Releaseには、native binaryに加えて、checksumとprovenance attestationを持つ
4つのplatform別MCPBを含めます。

```text
sippion-linux-x86_64.mcpb
sippion-windows-x86_64.mcpb
sippion-macos-aarch64.mcpb
sippion-macos-x86_64.mcpb
```

MCPB manifestはhost側に明示的なproject rootを要求し、同じlocal stdio serverを起動します。
Codex、Claude Code、Antigravity、OpenCodeの設定まで自動で行いたい場合は、上のbootstrap +
`sippion setup` 経路を引き続き推奨します。Registry/MCPBは、標準化された追加のdiscovery / install channelです。

## Sippionは何をするの？

AIクライアントは、たとえば次のようにSippionへ問い合わせます。

```text
repo_context {"q":"authentication token validation"}
```

Sippionはリポジトリの大部分をそのままAIへ渡すのではなく、関連するコード断片と
構造的な根拠を、上限付きで返します。内部のranking score（順位付けスコア）やbudget metadata
（予算管理用の付随情報）は、correctnessのために必要でない限りAIには見せません。
AIが受け取るのは、主にpath、line range、根拠コード、必要最小限の不完全検索状態です。

イメージは次のとおりです。

```text
AIコーディングエージェント
    ↓ どこを読むべきか問い合わせる
Sippion repo_context
    ↓ 関連箇所だけ返す
AIが必要なソースファイルを通常どおり読む
```

大きなリポジトリ、初見のコードベース、複数エージェントが並行して調査する場面で、
無駄な探索とコンテキスト消費を減らすために使います。

`session_id` と `agent_id` を指定すると、協調する複数エージェントの状態を
プロセスメモリ上で共有できます。この情報は永続化されません。

## Efficiency layer（効率化レイヤー）

同じ指示を何度もAIへ読ませないため、Sippionは責務を3つに分けます。

1. MCP server instruction（MCPサーバー指示）は、いつ `repo_context` を使い、いつ通常のfile readへ戻るかだけを伝えます。
2. `repo_context` はrankingの内部事情を見せず、次の判断に必要な最小のリポジトリ根拠を返します。
3. managed global rule（Sippionが管理する共通ルール）は、最小の正しい実装、既存コード再利用、不要な抽象化・選択肢の削減、安全性維持、短い回答を求めます。

このルールは常時有効です。lite/full/ultraのような切り替え、Node hook、実行時のupstream取得はありません。
詳しい設計は [Efficiency layer](docs/efficiency-layer.md) にあります。

トークン削減はcorrectnessを維持した場合だけ改善として扱います。
[Efficiency benchmark pilot](eval/efficiency/README.md) は4条件、task/armごと5回、
deterministic check（機械的に一意判定できる検証）とtotal model tokensだけで評価します。
LLM judge（別AIによる採点）は使いません。実モデルでこのprotocolを完走するまでは、削減率を公開値として主張しません。

## 安全性

Sippion本体は次の性質を持ちます。

- ローカルstdio MCP
- プロジェクト単位
- 読み取り専用
- リポジトリ文脈を返している間はネットワーク通信なし
- 検索状態はRAM上のみで、永続インデックスを作らない

Sippionは、リポジトリ内のコードを実行しません。モデル通信を中継せず、
プロバイダの認証情報を保持せず、デーモンを起動せず、リポジトリを書き換えません。

ファイル読み取りには上限があり、symlinkや危険なhard linkを拒否し、読み取り前後で
対象ファイルの同一性を再確認します。高確度のsecretは出力前にredactします。
また、リポジトリ内の文章は**命令ではなく信頼できないデータ**として扱います。

完全なtrust boundary（信頼境界）やインストール時の検証モデルは
[Security and trust boundary](docs/security.md) を参照してください。

## 対応AIクライアント

`sippion setup` は現在のユーザーに対して次の4つを設定します。

- Codex
- Claude Code
- Antigravity
- OpenCode

すでに起動しているクライアントは、MCP設定を読み直すためインストール後に再起動してください。

よく使うコマンド:

```sh
sippion setup
sippion doctor
sippion uninstall
```

`setup` は何度実行しても同じ状態に収束し、管理対象ファイル全体についてtransactionalに動作します。
また、Sippion管理ブロックのBEGIN/ENDマーカーが欠落・重複・逆順になっている場合は、
関係ないユーザー設定を巻き込まないよう**書き換えずエラー終了**します。
管理対象の設定ファイルだけでなく、管理対象の親ディレクトリがsymlink（別の場所を指すリンク）の場合も
書き換えを拒否します。Unix系ではMCP client config（MCPクライアント設定）を `0600`
（所有者だけが読み書き可能）に作成・補修し、rollback（失敗時の復元）でも元のpermission bits
（アクセス権）を戻します。永続的な `.sippion-backup` は新規作成せず、旧バージョンが残したものは
setup時にtransactionalに削除します。いずれかのクライアント設定に失敗した場合、そのsetup試行で
触れたファイルは開始前の状態へ戻します。

`doctor` は登録状態を診断し、MISSING / MISMATCH / ERRORが1件でもあれば非0終了します。
`uninstall` もtransactionalで、削除前に管理対象設定・ルールをsnapshot（開始前状態の退避）し、
途中で1件でも失敗した場合は開始前の状態へrollbackします。Sippionが管理しているクライアント設定と
ルールだけを削除し、関係のない設定やSippion本体のバイナリは触りません。

手動設定や診断の詳細は [Client setup](docs/clients.md) を参照してください。

## Sippionを手動起動する

現在ディレクトリから安全なproject root（プロジェクトの読み取り範囲）を自動推定する場合:

```sh
sippion mcp --root-auto
```

自動推定では最も近いGit/project marker（プロジェクト境界を示す目印）を採用します。
より近いmanifestを越えて外側の `.git` を優先することはなく、Unix系では他ユーザーや
グループが書き込める共有ディレクトリを自動境界として信頼しません。home directoryの解決自体も
安全性チェックの一部なので、homeを正規化できない場合はhome/ancestor guard（ホームやその親を拒否する防御）を
黙って無効化せず、自動探索を停止します。

Windowsでは `--root-auto` をcanonical current-user profile配下に限定します。それ以外の
信頼できるプロジェクトを使う場合は明示的に指定してください。

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
```

home directory、filesystem root、またはhome directoryの親ディレクトリを明示rootにする操作は
デフォルトで拒否します。本当に広域走査を意図する手動実行だけ、`--allow-broad-root` を明示してください。
`sippion setup` がこのoverride（安全制限の明示解除）を自動設定することはありません。

adaptive scan ceiling（適応的な走査上限）を下げる場合:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

## 仕組み

検索はRAM上の字句インデックスから始まり、直前の探索拡張で有用な根拠が増えている間だけ
走査範囲を広げます。上位候補を構文解析し、importや意味関係から得た隣接候補も決定的な順位で
上限付き追加を行います。最後に、検証済みコード断片と構造情報を「推定1トークンあたりの有用性」で
選び、同じファイルの重複情報には割引をかけてAIへ渡すコンテキストを小さくします。
推定トークン数はsoft target（目標値）で、独立したbyte上限がmodel-visible output（AIに見える出力）の
hard guard（絶対上限）です。検索語の大小文字処理はUnicode対応ですが、filesystem safety policy
（ファイルシステム安全規則）は検索品質とは分離した保守的な判定を維持します。

Sippionはリポジトリ文脈を絞り込むツールであり、コンパイラやLanguage Serverではありません。
コンパイラ相当の型解決や、LSP相当の参照解決を保証するものではありません。

詳しくは [Architecture](docs/architecture.md) と
[Integration boundaries](docs/integrations.md) を参照してください。

## 開発

Rust 1.85.0を固定しており、`Cargo.lock` もコミットしています。

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
python3 eval/efficiency/score_test.py
```

CIではさらに `Cargo.lock` をRustSec advisory databaseに対して監査します。

生成される実行ファイルは、Unix系では `target/release/sippion`、Windowsでは
`target/release/sippion.exe` です。

## メンテナ向け: Release

配布対象のバイナリは次の4つです。

```text
sippion-linux-x86_64
sippion-windows-x86_64.exe
sippion-macos-aarch64
sippion-macos-x86_64
```

Release workflowは4ターゲットをbuildし、portableなSHA-256ファイルと
GitHub Artifact Attestationを生成します。サードパーティGitHub Actionsはfull commit SHAに固定します。
Pull RequestのRelease supply-chain smokeでは配布用Attestationを新規発行せずにbuild/assemblyを検証し、
別ジョブで公開済みinstallerとbinaryを、実際のinstallerと同じrepository + signer workflow + source SHA条件で
consumer側から検証します。

version bumpが `main` に入った後、prereleaseを自動公開する場合は、current `main` と
完全に同じcommitを指す `release/vX.Y.Z[-prerelease]` の一時branchを作ります。
workflowがversionとtagを検証してprereleaseを公開し、成功後にそのbranchを削除します。
手動draft releaseは、入力したtagと同じtag refからworkflowを起動しなければ拒否されます。

安定版（例: `v0.1.0`）を切る場合: `Cargo.toml` を正確なversionに上げて `main` にmergeし、
current `main` を指す一時branch `release/v0.1.0` をpushします。workflowの公開を待ってから
prerelease扱いを外します。

```sh
gh release edit v0.1.0 --repo Sitten-Tokyo/Sippion --prerelease=false
```

## Credits

Sippionのcompact efficiency behavior（小さな効率化ルール）は
[Ponytail](https://github.com/DietrichGebert/ponytail) と
[i-have-adhd](https://github.com/ayghri/i-have-adhd) にInspired by（着想を得ています）。
両プロジェクトをvendorせず、Sippion向けに書き直した常時有効ルールを使用します。
確認済みupstream commitは `upstream.toml` に固定し、ライセンス情報は
[Third-party notices](THIRD_PARTY_NOTICES.md) に記載しています。

## ドキュメント

- [English README](README.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Efficiency layer](docs/efficiency-layer.md)
- [Efficiency benchmark pilot](eval/efficiency/README.md)
- [Integration boundaries](docs/integrations.md)
- [Historical RC changes and validation](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
