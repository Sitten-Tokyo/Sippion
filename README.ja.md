# Sippion

[![Release](https://img.shields.io/github/v/release/Sitten-Tokyo/Sippion?label=release)](https://github.com/Sitten-Tokyo/Sippion/releases)
[![CI](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml/badge.svg)](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.Sitten--Tokyo%2Fsippion-blue)](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)](THIRD_PARTY_NOTICES.md)
[![Rust](https://img.shields.io/badge/rust-1.85-orange)](rust-toolchain.toml)

[English](README.md) | **日本語**

**聞くべき場所を先に絞る。読むのは3ファイルでいい。**

Sippionは、AIコーディングエージェント向けのローカル・読み取り専用MCPサーバーです。
公開ツールは `repo_context` の1つだけ。エージェントが大量のファイルを開く前に、
関連するソースコードを小さな断片へ絞り込みます。

- MCPツールは `repo_context` の1つ
- ローカルstdio（標準入出力）・読み取り専用
- リポジトリ文脈の提供中はネットワーク通信なし
- 検索状態はRAMのみで、永続インデックスを作らない
- 字句・構造・意味情報を組み合わせた上限付き検索
- 高確度のsecret（機密情報）を出力前にredact（伏せ字化）
- Codex / Claude Code / Antigravity / OpenCode に対応

## インストール

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.ps1 | iex
```

インストール後はAIクライアントを再起動してください。`sippion setup` は対応する4クライアントへ
Sippionを事前登録し、`--root-auto` で最も近い安全なproject boundary（プロジェクト境界）を
自動選択します。home directory（ホームディレクトリ）やfilesystem root（ファイルシステムの最上位）は
自動選択しません。

GitHub Artifact Attestation（配布物の出所証明）まで厳密に検証する場合は、
インストーラー実行前に `SIPPION_STRICT_PROVENANCE=1` を設定してください。
詳細は [Security and trust boundary](docs/security.md) にあります。

Official MCP Registryには `io.github.Sitten-Tokyo/sippion` として公開しています。
[最新のRegistry entry](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
から確認できます。

## 使い方

AIエージェントは、まずSippionへ対象箇所を問い合わせます。

```text
repo_context {"q":"authentication token validation"}
```

Sippionは関連する少数のコード断片と構造的な根拠だけを返します。
その後、エージェントは必要なソースファイルだけを通常どおり読みます。

```text
AIコーディングエージェント
    ↓ 検索範囲を絞る
Sippion repo_context
    ↓ 必要な根拠だけ返す
AIが関連ソースだけ読む
```

`session_id` と `agent_id` を指定すると、複数エージェント間でプロセスメモリ上の
協調情報を共有できます。永続化はされません。

よく使うコマンド:

```sh
sippion setup
sippion doctor
sippion uninstall
sippion mcp --root-auto
```

信頼済みのプロジェクトを明示する場合:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
```

home directoryやfilesystem rootのような広すぎるrootは既定で拒否されます。
意図的に広い範囲を読む場合だけ `--allow-broad-root` が必要です。`setup` はこの設定を有効にしません。

## Sippionを使う理由

|  | Grep / Glob | 常駐インデックス型 | Sippion |
|---|---|---|---|
| 導入 | なし | デーモン＋ディスク索引 | 1コマンド |
| 新鮮さ | 常に最新 | 再索引ラグあり | 常に最新 |
| AIへ渡る情報 | 素の一致 | 広くなりやすい | 上限付き断片 |
| 提供中の通信 | n/a | 場合による | なし |
| リポジトリへの書込 | なし | 場合による | なし |

Sippionはrepository-context tool（リポジトリ文脈を絞る道具）であり、compiler（コンパイラ）や
language server（言語サーバー）ではありません。意味情報は上限付きの補助情報であり、
compiler-authoritative（コンパイラが保証する厳密な型・参照解決）とは主張しません。

構造解析はRust、Python、JavaScript/TypeScript、Go、Java、C#、C、C++に対応しています。

## 安全性

Sippionは検索中に、リポジトリ内のコード、build script、compiler、LSP server、shell commandを
実行しません。モデル通信を中継せず、provider credential（AIサービスの認証情報）を保持せず、
デーモンを起動せず、リポジトリを書き換えません。

リポジトリ内の文章は**命令ではなく、信頼できないデータ**として扱います。
読み取り範囲と出力量には上限があり、危険なリンクを拒否し、読み取り前後で対象ファイルの同一性を
再確認し、高確度のsecretをAIへ渡す前にredactします。

完全なtrust boundary（信頼境界）は [Security and trust boundary](docs/security.md) を参照してください。

## Efficiency rule（効率化ルール）

`sippion setup` は、対応エージェントへ小さな常時有効ルールを設定します。
最小の正しい実装、既存コードの再利用、不要な抽象化の回避、安全性の維持、短いユーザー向け出力を
優先させます。lite/full/ultraのようなモード切替や、実行時のprompt downloadはありません。

トークン効率は、deterministic check（機械的に再現可能な正誤判定）を通過してcorrectness（正しさ）が
維持された場合だけ改善として評価します。詳しくは [Efficiency layer](docs/efficiency-layer.md) と
[benchmark pilot](eval/efficiency/README.md) を参照してください。

## 開発

Rust 1.85.0を固定し、`Cargo.lock` をコミットしています。

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
python3 eval/efficiency/score_test.py
```

release binary（配布用バイナリ）は `target/release/sippion`、Windowsでは `sippion.exe` です。
Release workflowはLinux x86_64、Windows x86_64 MSVC、macOS Apple Silicon、macOS Intel向けを
ビルドし、checksum（ハッシュ値による整合性確認）とprovenance attestation（出所証明）を生成します。

retrieval（検索）、security、distribution（配布）、workflowを変更する前に
[CONTRIBUTING.md](CONTRIBUTING.md) を確認してください。

## ドキュメント

- [English README](README.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Efficiency layer](docs/efficiency-layer.md)
- [Quality and validation](docs/quality.md)
- [Integration boundaries](docs/integrations.md)
- [Historical changes](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)

## Credits

効率化ルールの発想は [Ponytail](https://github.com/DietrichGebert/ponytail) と
[i-have-adhd](https://github.com/ayghri/i-have-adhd) を参考にしています。
確認済みのupstream commitは `upstream.toml`、ライセンス情報は
[Third-party notices](THIRD_PARTY_NOTICES.md) に記載しています。
