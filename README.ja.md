# Sippion

[English](README.md) | **日本語**

Sippion は、AIコーディングエージェントがリポジトリ全体を闇雲に読む前に、
**必要そうな箇所を絞り込むためのローカル・読み取り専用MCPサーバー**です。

公開するツールは `repo_context` の1つだけです。字句検索、構造情報、
ソースコードだけを対象にした意味ランキングを組み合わせ、関連度の高い小さな
コード断片を返します。

## まずはインストール

1コマンドでSippionをインストールできます。checksumを検証したうえで、
`sippion setup` まで自動実行されます。GitHubへのログインは不要です。

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/e69e13c9f34710e953722c11628b9f50df93bb7f/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/e69e13c9f34710e953722c11628b9f50df93bb7f/scripts/bootstrap.ps1 | iex
```

インストール後はこうなります。

```text
Sippionをインストール
    ↓
Codex + Claude Code + Antigravity に事前登録
    ↓
各AIクライアントを再起動
    ↓
プロジェクトを開けばSippionが使える
```

Sippionは、**3クライアントすべてに事前登録**します。今そのクライアントが
インストールされていなくても設定は作られます。

各クライアントはSippionを `--root .` で起動するため、そのクライアントで開いた
プロジェクトがSippionの読み取り専用ルートになります。リポジトリごとにSippionを
登録し直す必要はありません。

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

完全なtrust boundary（信頼境界）や、Artifact Attestationまで検証するより厳格な
インストール方法は [Security and trust boundary](docs/security.md) を参照してください。

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

`setup` は何度実行しても同じ状態に収束します。`doctor` は登録状態を診断します。
`uninstall` はSippionが管理しているクライアント設定とルールだけを削除し、
関係のない設定は触りません。

手動設定や診断の詳細は [Client setup](docs/clients.md) を参照してください。

## Sippionを手動起動する

特定のプロジェクトを明示的にルートとして起動する場合:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
```

adaptive scan ceiling（適応的な走査上限）を下げる場合:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

## 仕組み

検索はRAM上の字句インデックスから始まり、上位候補だけを構文解析し、
ソースコードだけを対象にした意味的な根拠を追加して、検証済みの断片を上限内にまとめます。

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
GitHub Artifact Attestationを生成します。サードパーティGitHub Actionsはfull commit SHAに固定し、
Release supply-chain smoke workflowで、Releaseを公開せずにbuild、Attestation、
artifact upload/download、installer Attestationまで検証します。

version bumpが `main` に入った後、prereleaseを自動公開する場合は、current `main` と
完全に同じcommitを指す `release/vX.Y.Z[-prerelease]` の一時branchを作ります。
workflowがversionとtagを検証してprereleaseを公開し、成功後にそのbranchを削除します。

## ドキュメント

- [English README](README.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Integration boundaries](docs/integrations.md)
- [Historical RC changes and validation](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
