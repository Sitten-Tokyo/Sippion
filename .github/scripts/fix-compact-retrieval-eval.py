#!/usr/bin/env python3
from pathlib import Path

path = Path("scripts/retrieval-eval.py")
source = path.read_text(encoding="utf-8")

constant_prefix = "ATOM_HEADER = re.compile("
matching_lines = [line for line in source.splitlines() if line.startswith(constant_prefix)]
if len(matching_lines) != 1:
    raise SystemExit(f"expected exactly one ATOM_HEADER declaration, found {len(matching_lines)}")

constants = r'''LEGACY_ATOM_HEADER = re.compile(r'^(S|E) path=("(?:\\.|[^"\\])*")')
COMPACT_EVIDENCE_HEADER = re.compile(r'^("(?:\\.|[^"\\])*"):\d+-\d+\s*$')
COMPACT_PATH_HEADER = re.compile(r'^("(?:\\.|[^"\\])*")\s*$')'''
source = source.replace(matching_lines[0], constants, 1)

start_marker = "def parse_context_atoms(context):\n"
end_marker = "\n\ndef normalized_requirements(case):"
if source.count(start_marker) != 1 or source.count(end_marker) != 1:
    raise SystemExit("unexpected parse_context_atoms layout")
start = source.index(start_marker)
end = source.index(end_marker, start)

new_function = '''def parse_context_atoms(context):
    """Parse both legacy labeled atoms and the compact model-visible format."""
    atoms = []
    current = None

    def begin(kind, encoded_path, line):
        try:
            decoded_path = json.loads(encoded_path)
        except json.JSONDecodeError:
            decoded_path = "<invalid>"
        return {"kind": kind, "path": decoded_path, "text": line}

    for line in context.splitlines(keepends=True):
        bare = line.rstrip("\\r\\n")
        legacy = LEGACY_ATOM_HEADER.match(bare)
        compact_evidence = COMPACT_EVIDENCE_HEADER.match(bare)
        compact_path = COMPACT_PATH_HEADER.match(bare)

        if legacy:
            if current is not None:
                atoms.append(current)
            current = begin(
                "structure" if legacy.group(1) == "S" else "evidence",
                legacy.group(2),
                line,
            )
            continue

        if compact_evidence:
            if current is not None:
                atoms.append(current)
            current = begin("evidence", compact_evidence.group(1), line)
            continue

        if compact_path:
            if current is not None:
                atoms.append(current)
            # A bare compact path can represent either a structure atom or path-only
            # evidence. Classify it from its first continuation line when possible.
            current = begin("unknown", compact_path.group(1), line)
            continue

        if current is not None:
            if current["kind"] == "unknown":
                if line.startswith("| "):
                    current["kind"] = "evidence"
                elif line.startswith("  "):
                    current["kind"] = "structure"
            current["text"] += line

    if current is not None:
        atoms.append(current)
    return atoms
'''
source = source[:start] + new_function + source[end:]
path.write_text(source, encoding="utf-8")
