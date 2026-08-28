#!/usr/bin/env python3
import argparse
import json
import os
import shutil
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True)
parser.add_argument('--platform', choices=['linux', 'darwin', 'win32'], required=True)
parser.add_argument('--version', required=True)
parser.add_argument('--output-dir', required=True)
args = parser.parse_args()

output = Path(args.output_dir)
server = output / 'server'
server.mkdir(parents=True, exist_ok=True)
binary_name = 'sippion.exe' if args.platform == 'win32' else 'sippion'
destination = server / binary_name
shutil.copy2(args.binary, destination)
if args.platform != 'win32':
    os.chmod(destination, 0o755)

manifest = {
    'manifest_version': '0.3',
    'name': 'sippion',
    'display_name': 'Sippion',
    'version': args.version,
    'description': 'Local read-only MCP repository context retrieval for AI coding agents.',
    'long_description': 'Sippion narrows repository-wide discovery to bounded ranked structural context before agents broadly open source files. It is local-only, read-only, no-network while serving, and project-scoped.',
    'author': {'name': 'Sitten-Tokyo'},
    'repository': {'type': 'git', 'url': 'https://github.com/Sitten-Tokyo/Sippion.git'},
    'documentation': 'https://github.com/Sitten-Tokyo/Sippion#readme',
    'support': 'https://github.com/Sitten-Tokyo/Sippion/issues',
    'server': {
        'type': 'binary',
        'entry_point': f'server/{binary_name}',
        'mcp_config': {
            'command': f'${{__dirname}}/server/{binary_name}',
            'args': ['mcp', '--root', '${user_config.project_root}'],
            'env': {},
        },
    },
    'tools': [{
        'name': 'repo_context',
        'description': 'Return bounded ranked structural repository context and source evidence.',
    }],
    'keywords': ['repository', 'coding-agent', 'context', 'search', 'local', 'read-only'],
    'license': 'MIT OR Apache-2.0',
    'compatibility': {'platforms': [args.platform]},
    'user_config': {
        'project_root': {
            'type': 'directory',
            'title': 'Project root',
            'description': 'Repository or project directory that Sippion may read.',
            'required': True,
        },
    },
}
(output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
