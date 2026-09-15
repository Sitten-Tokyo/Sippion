# Sippion

[![Release](https://img.shields.io/github/v/release/Sitten-Tokyo/Sippion?label=release)](https://github.com/Sitten-Tokyo/Sippion/releases)
[![CI](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml/badge.svg)](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.Sitten--Tokyo%2Fsippion-blue)](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)](THIRD_PARTY_NOTICES.md)
[![Rust](https://img.shields.io/badge/rust-1.85-orange)](rust-toolchain.toml)

[English](README.md) | **日本語**

**AIに渡す文脈を減らし、APIトークンもサブスクリプションの利用枠も長持ちさせる。**

Sippionは、AIコーディングエージェントのトークン消費を抑えるための、ローカル・読み取り専用MCPサーバーです。エージェントがリポジトリ全体を広く読む代わりに、単一ツール `repo_context` が関連するソースコードだけを小さな断片に絞って返します。

その結果、1タスクあたりの不要な文脈を減らせます。API利用時のトークン消費を抑えられるだけでなく、**サブスクリプション型のAIコーディングサービスでも、必要な文脈量を減らすことで利用枠を長持ちさせやすくなる**のがSippionの大きな違いです。

## 仕組み

```text
AIコーディングエージェント
    ↓ 必要なリポジトリ文脈を問い合わせる
Sippion repo_context
    ↓ 関連する少数の断片だけ返す
AIが必要なファイルだけ読む
```

例:

```text
repo_context {"q":"authentication token validation"}
```

Sippionは、字句・構造・意味情報を組み合わせた上限付き検索を、ローカルかつ読み取り専用で行います。

## インストール

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.ps1 | iex
```

インストール後にAIクライアントを再起動してください。`sippion setup` でCodex、Claude Code、Antigravity、OpenCodeへ登録できます。

よく使うコマンド:

```sh
sippion setup
sippion doctor
sippion uninstall
sippion mcp --root-auto
```

## Sippionの特徴

- **トークン消費を削減:** 広いファイル読み取りの前に、必要なリポジトリ文脈へ絞り込む。
- **API以外にも有効:** サブスクリプションの利用枠も長持ちさせやすい。
- **ローカル:** リポジトリ文脈の提供中はネットワーク通信なし。
- **読み取り専用:** リポジトリを書き換えず、リポジトリ内のコードも実行しない。
- **永続インデックスなし:** 検索状態はRAMのみ。
- **単一ツール:** 公開MCPツールは `repo_context` の1つだけ。

Sippionはリポジトリ文脈を絞るツールであり、コンパイラや言語サーバーではありません。構造解析はRust、Python、JavaScript/TypeScript、Go、Java、C#、C、C++に対応しています。

## 安全性

検索中にリポジトリ内のコード、ビルドスクリプト、コンパイラ、LSPサーバー、シェルコマンドを実行しません。リポジトリ内の文章は信頼できないデータとして扱い、高確度の機密情報はAIへ渡す前に伏せ字化します。

詳しくは [Security and trust boundary](docs/security.md) を参照してください。

## ドキュメント

- [Client setup](docs/clients.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Efficiency layer](docs/efficiency-layer.md)
- [Quality and validation](docs/quality.md)
- [Contributing](CONTRIBUTING.md)

## Credits

効率化ルールの発想は [Ponytail](https://github.com/DietrichGebert/ponytail) と [i-have-adhd](https://github.com/ayghri/i-have-adhd) を参考にしています。ライセンス情報は [Third-party notices](THIRD_PARTY_NOTICES.md) に記載しています。
