# Sippion

[English](README.md) | **日本語**

Sippion は、AIコーディングエージェントがリポジトリ全体を闇雲に読む前に、
**必要そうな箇所を絞り込むためのローカル・読み取り専用MCPサーバー**です。

公開するツールは `repo_context` の1つだけです。字句検索、構造情報、
ソースコードだけを対象にした意味ランキングを組み合わせ、関連度の高い小さな
コード断片を返します。

## まずはインストール

1コマンドでSippionをインストールできます。bootstrapは取得したinstallerについて、
checksumに加えて **GitHub Artifact Attestation（その成果物が正規のGitHub Actionsから生成されたことの証明）**
を**実行前に検証**します。その後installerが、選択したバイナリのchecksumと
Artifact Attestationを検証してからインストールし、transactional（途中失敗時に元へ戻す方式）の
`sippion setup` まで自動実行します。

デフォルト経路は、GitHub CLI (`gh`) がインストール済みで、`gh attestation` に対応し、
GitHubへ認証できない場合はfail closed（安全性を確認できなければ処理を止める方式）で停止します。
メインのインストール導線ではprovenance（成果物の出所保証）を無効化しません。

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
installer checksum + GitHub Artifact Attestationを検証
    ↓
バイナリchecksum + GitHub Artifact Attestationを検証
    ↓
Sippionをインストール
    ↓
Codex + Claude Code + Antigravity に事前登録
    ↓
各AIクライアントを再起動
```

2段階のAttestation検証はいずれも、Sippionリポジトリだけでなく、期待するRelease workflowと、
選択したRelease tagから解決した正確なcommit SHAまで固定して確認します。

Sippionは、**3クライアントすべてに事前登録**します。今そのクライアントが
インストールされていなくても設定は作られます。

各クライアントはSippionを `--root-auto` で起動します。Sippionは現在位置から最も近い
Git/project boundary（Gitまたはproject manifestで示されるプロジェクト境界）をルートに選びます。
外側の `.git` を探すために、より近いproject manifestを越えて探索範囲を広げることはありません。
またUnix系では、group/other-writable directory（同じグループや他ユーザーが書き込める共有ディレクトリ）を
自動境界として信頼しません。ユーザーのhome directory（ホームディレクトリ）やfilesystem root
（ファイルシステム最上位）を自動選択する場合もfail closedで拒否します。
Windowsではstable safe Rust API（安定版Rustの安全なAPI）だけでは任意の共有ディレクトリのACL
（アクセス制御リスト）を十分に検証できないため、`--root-auto` はcanonical current-user profile
（正規化された現在ユーザーのプロファイル）配下に限定します。それ以外の信頼できるプロジェクトは
明示的な `--root` で指定できます。通常の自動探索範囲では、リポジトリごとにSippionを登録し直す必要はありません。

別の信頼できる方法でprovenanceを検証済みの管理環境向けには、checksumのみで進める
明示的なopt-out（利用者が意図して検証を外す設定）もdirect installerに残しています。
詳細は [Security and trust boundary](docs/security.md) を参照してください。

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
Codex、Claude Code、Antigravityの設定まで自動で行いたい場合は、上のbootstrap + `sippion setup`
経路を引き続き推奨します。Registry/MCPBは、標準化された追加のdiscovery / install channelです。

## Sippionは何をするの？

AIクライアントは、たとえば次のようにSippionへ問い合わせます。

```text
repo_context {"q":"authentication token validation"}
```

Sippionはリポジトリの大部分をそのままAIへ渡すのではなく、関連するコード断片と
構造的な根拠を、上限付きで返します。

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

`sippion setup` は現在のユーザーに対して次の3つを設定します。

- Codex
- Claude Code
- Antigravity

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

検索はRAM上の字句インデックスから始まり、上位候補だけを構文解析し、
ソースコードだけを対象にした意味的な根拠を追加して、検証済みの断片を上限内にまとめます。
検索語の大小文字処理はUnicode対応ですが、filesystem safety policy（ファイルシステム安全規則）は
検索品質とは分離した保守的な判定を維持します。

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

## ドキュメント

- [English README](README.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Integration boundaries](docs/integrations.md)
- [Historical RC changes and validation](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
