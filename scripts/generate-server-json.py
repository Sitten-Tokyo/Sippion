#!/usr/bin/env python3
import argparse
import hashlib
import json
from pathlib import Path

REPOSITORY_URL = 'https://github.com/Sitten-Tokyo/Sippion.git'
REPOSITORY_ID = '1338733125'
WEBSITE_URL = 'https://github.com/Sitten-Tokyo/Sippion'
ICON_URL = 'https://github.com/Sitten-Tokyo.png?size=128'

parser = argparse.ArgumentParser()
parser.add_argument('--assets-dir', required=True)
parser.add_argument('--tag', required=True)
parser.add_argument('--version', required=True)
parser.add_argument('--output', required=True)
args = parser.parse_args()
assets = Path(args.assets_dir)
names = [
    'sippion-linux-x86_64.mcpb',
    'sippion-windows-x86_64.mcpb',
    'sippion-macos-aarch64.mcpb',
    'sippion-macos-x86_64.mcpb',
]
packages = []
for name in names:
    content = (assets / name).read_bytes()
    digest = hashlib.sha256(content).hexdigest()
    packages.append({
        'registryType': 'mcpb',
        'identifier': f'https://github.com/Sitten-Tokyo/Sippion/releases/download/{args.tag}/{name}',
        'fileSha256': digest,
        'transport': {'type': 'stdio'},
    })
value = {
    '$schema': 'https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json',
    'name': 'io.github.Sitten-Tokyo/sippion',
    'title': 'Sippion',
    'description': 'Local read-only repository context retrieval for AI coding agents.',
    'repository': {
        'url': REPOSITORY_URL,
        'source': 'github',
        'id': REPOSITORY_ID,
    },
    'websiteUrl': WEBSITE_URL,
    'icons': [{
        'src': ICON_URL,
        'mimeType': 'image/png',
    }],
    'version': args.version,
    'packages': packages,
}
Path(args.output).write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')
