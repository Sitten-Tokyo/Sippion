#!/usr/bin/env python3
from pathlib import Path

path = Path(__file__).resolve().parents[1] / "deny.toml"
text = path.read_text(encoding="utf-8")
old = "skip = []"
new = '''skip = [
  { crate = "io-lifetimes@2.0.4", reason = "fs-set-times in cap-primitives 4.0.2 still requires io-lifetimes 2" },
  { crate = "windows-sys@0.59.0", reason = "cap-primitives 4.0.2 Windows support still requires this line" },
  { crate = "windows-sys@0.60.2", reason = "io-extras 0.19.0 still requires this line" },
  { crate = "windows-targets@0.52.6", reason = "windows-sys 0.59.0 still requires this target bundle" },
  { crate = "windows_aarch64_gnullvm@0.52.6", reason = "transitive via windows-targets 0.52.6" },
  { crate = "windows_aarch64_msvc@0.52.6", reason = "transitive via windows-targets 0.52.6" },
  { crate = "windows_i686_gnu@0.52.6", reason = "transitive via windows-targets 0.52.6" },
  { crate = "windows_i686_gnullvm@0.52.6", reason = "transitive via windows-targets 0.52.6" },
  { crate = "windows_i686_msvc@0.52.6", reason = "transitive via windows-targets 0.52.6" },
  { crate = "windows_x86_64_gnu@0.52.6", reason = "transitive via windows-targets 0.52.6" },
  { crate = "windows_x86_64_gnullvm@0.52.6", reason = "transitive via windows-targets 0.52.6" },
  { crate = "windows_x86_64_msvc@0.52.6", reason = "transitive via windows-targets 0.52.6" },
]'''
if text.count(old) != 1:
    raise SystemExit(f"deny.toml: expected one skip=[] target, got {text.count(old)}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
print("explicit duplicate exceptions applied")
