#!/usr/bin/env python3
import argparse
import shutil
import stat
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

FIXED_ZIP_TIME = (2000, 1, 1, 0, 0, 0)
MAX_TOTAL_UNCOMPRESSED = 128 * 1024 * 1024


def safe_name(name: str) -> PurePosixPath:
    if not name or "\\" in name or name.startswith("/"):
        raise ValueError(f"unsafe MCPB entry name: {name!r}")
    path = PurePosixPath(name)
    if any(part in ("", ".", "..") for part in path.parts):
        raise ValueError(f"unsafe MCPB entry name: {name!r}")
    return path


def canonical_mode(path: PurePosixPath, is_dir: bool) -> int:
    if is_dir:
        return stat.S_IFDIR | 0o755
    if len(path.parts) == 2 and path.parts[0] == "server" and path.name.startswith("sippion"):
        return stat.S_IFREG | 0o755
    return stat.S_IFREG | 0o644


def canonicalize(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(source, "r") as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        if len(names) != len(set(names)):
            raise ValueError("MCPB contains duplicate ZIP entry names")

        entries = []
        total_size = 0
        for info in infos:
            path = safe_name(info.filename.rstrip("/"))
            unix_mode = (info.external_attr >> 16) & 0xFFFF
            if stat.S_ISLNK(unix_mode):
                raise ValueError(f"MCPB contains symlink entry: {info.filename}")
            is_dir = info.is_dir()
            data = b"" if is_dir else archive.read(info)
            total_size += len(data)
            if total_size > MAX_TOTAL_UNCOMPRESSED:
                raise ValueError("MCPB exceeds canonicalization size limit")
            entries.append((path, is_dir, data))

    if not any(path.as_posix() == "manifest.json" and not is_dir for path, is_dir, _ in entries):
        raise ValueError("MCPB is missing manifest.json")
    binaries = [
        path
        for path, is_dir, _ in entries
        if not is_dir and len(path.parts) == 2 and path.parts[0] == "server" and path.name in ("sippion", "sippion.exe")
    ]
    if len(binaries) != 1:
        raise ValueError(f"MCPB must contain exactly one Sippion server binary, found {len(binaries)}")

    with tempfile.NamedTemporaryFile(
        dir=destination.parent,
        prefix=f".{destination.name}.",
        suffix=".tmp",
        delete=False,
    ) as handle:
        temp_path = Path(handle.name)

    try:
        with zipfile.ZipFile(
            temp_path,
            "w",
            compression=zipfile.ZIP_DEFLATED,
            compresslevel=9,
            strict_timestamps=True,
        ) as output:
            output.comment = b""
            for path, is_dir, data in sorted(entries, key=lambda item: item[0].as_posix()):
                name = path.as_posix() + ("/" if is_dir else "")
                info = zipfile.ZipInfo(name, FIXED_ZIP_TIME)
                info.create_system = 3
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = canonical_mode(path, is_dir) << 16
                if is_dir:
                    info.external_attr |= 0x10
                info.extra = b""
                info.comment = b""
                output.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)

        with zipfile.ZipFile(temp_path, "r") as check:
            if check.testzip() is not None:
                raise ValueError("canonical MCPB ZIP integrity check failed")
        shutil.move(temp_path, destination)
    finally:
        temp_path.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    source = Path(args.input)
    destination = Path(args.output)
    if source.resolve() == destination.resolve():
        raise SystemExit("--input and --output must be different paths")
    canonicalize(source, destination)


if __name__ == "__main__":
    main()
