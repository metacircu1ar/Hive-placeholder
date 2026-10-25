"""Shared, unversioned release layout and native dependency inspection."""
import hashlib
import json
import platform
import re
import struct
import subprocess
from pathlib import Path
from .files import atomic_write_json

ROOT = Path(__file__).resolve().parents[2]
PARTS = {
    "hive": ("hive", [], ["hivectl", "hive-daemon"], "comb"),
    "hive-core": ("hive", ["--no-default-features"], ["hivectl", "hive-daemon"], "core"),
    "comb": ("comb", [], ["comb"], "http"),
    "hive-install": ("hive-install", [], ["hive-install"], None),
}


def run(args, **kwargs):
    return subprocess.check_output(args, cwd=ROOT, text=True, **kwargs).strip()


def digest(path):
    with Path(path).open("rb") as stream:
        value = hashlib.sha256()
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def native_target():
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(platform.machine())
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(platform.system())
    if not arch or not suffix:
        raise ValueError("Only native macOS and GNU/Linux arm64/x86_64 releases are supported")
    return f"{arch}-{suffix}"


def inspect_binary(path, target):
    """Reject non-native files and non-system dynamic dependency closures."""
    with Path(path).open("rb") as stream:
        header = stream.read(32)
    arm = target.startswith("aarch64-")
    if "apple" in target:
        if header[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", header, 4)[0] != (0x100000C if arm else 0x1000007):
            raise ValueError(f"Wrong Mach-O architecture: {path}")
        libraries = [line.strip().split(" (", 1)[0] for line in run(["otool", "-L", str(path)]).splitlines()[1:]]
        if any(not name.startswith(("/usr/lib/", "/System/Library/")) for name in libraries):
            raise ValueError(f"Non-system shared library in {path}: {libraries}")
        return {"libraries": libraries, "required_glibc": None}
    if header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != (183 if arm else 62):
        raise ValueError(f"Wrong ELF architecture: {path}")
    listing = run(["ldd", str(path)])
    libraries = []
    for line in listing.splitlines():
        if "linux-vdso" in line:
            continue
        name = line.split("=>", 1)[-1].strip().split(" (", 1)[0]
        if not name.startswith(("/lib/", "/lib64/", "/usr/lib/")):
            raise ValueError(f"Missing or non-system dependency: {line}")
        libraries.append(name)
    symbols = run(["readelf", "--version-info", str(path)])
    versions = re.findall(r"\bGLIBC_([0-9]+\.[0-9]+(?:\.[0-9]+)?)\b", symbols)
    if not versions:
        raise ValueError("GNU/Linux release lacks a verifiable GLIBC requirement")
    # Release programs are Rust with C dependencies only; a C++ runtime would be a
    # requirement the installer does not check.
    if re.search(r"\b(GLIBCXX|CXXABI)_[0-9]", symbols) or "libstdc++" in listing:
        raise ValueError(f"Release binaries must not require the C++ runtime: {path}")
    return {"libraries": libraries,
            "required_glibc": max(versions, key=lambda v: tuple(map(int, v.split('.'))))}


def source_identity():
    tracked = run(["git", "ls-files", "-z"]).split("\0")
    # New source is included before a stage commit; unrelated untracked plans are not.
    extra = run(["git", "ls-files", "--others", "--exclude-standard", "-z"]).split("\0")
    files = sorted(set(tracked + [p for p in extra if p.endswith((".rs", ".toml", ".c", ".h", ".py", ".sh", ".json", ".txt"))]))
    value = hashlib.sha256()
    for name in files:
        path = ROOT / name
        if name and path.is_file():
            value.update(name.encode() + b"\0" + bytes.fromhex(digest(path)))
    return {"commit": run(["git", "rev-parse", "HEAD"]), "dirty": bool(run(["git", "status", "--porcelain"])), "tree_sha256": value.hexdigest()}


def write_json(path, value):
    atomic_write_json(path, value, mode=0o644)
