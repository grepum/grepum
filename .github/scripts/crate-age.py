#!/usr/bin/env python3
"""Fail if Cargo.lock pins a crates.io version younger than the cooldown.

The window is read from `registry.global-min-publish-age` in .cargo/config.toml,
so Cargo's resolver and this check always agree. Cargo keeps versions that are
already in Cargo.lock even when they are too new (and older Cargo ignores the
setting), so this is what actually guarantees a release never ships one.
Publish times come from the crates.io sparse index (`pubtime`), not the API.

    python3 .github/scripts/crate-age.py
"""
import concurrent.futures
import datetime as dt
import json
import pathlib
import re
import sys
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
UNITS = {"second": 1, "minute": 60, "hour": 3600, "day": 86400, "week": 7 * 86400, "month": 30 * 86400}


def min_age() -> dt.timedelta:
    text = (ROOT / ".cargo" / "config.toml").read_text()
    m = re.search(r'^\s*global-min-publish-age\s*=\s*"([^"]*)"', text, re.M)
    if not m:
        sys.exit("registry.global-min-publish-age is not set in .cargo/config.toml")
    value = m.group(1).strip()
    if value == "0":
        return dt.timedelta(0)
    n = re.fullmatch(r"(\d+)\s*([a-z]+?)s?", value)
    if not n or n.group(2) not in UNITS:
        sys.exit(f"cannot parse min-publish-age {value!r}")
    return dt.timedelta(seconds=int(n.group(1)) * UNITS[n.group(2)])


def locked_packages():
    """(name, version) for every crates.io package in Cargo.lock."""
    for block in (ROOT / "Cargo.lock").read_text().split("[[package]]")[1:]:
        fields = dict(re.findall(r'^(name|version|source) = "([^"]*)"', block, re.M))
        if fields.get("source") == CRATES_IO:
            yield fields["name"], fields["version"]


def index_url(name: str) -> str:
    n = name.lower()
    prefix = {1: "1", 2: "2", 3: f"3/{n[0]}"}.get(len(n), f"{n[:2]}/{n[2:4]}")
    return f"https://index.crates.io/{prefix}/{n}"


def pubtime(pkg):
    name, version = pkg
    with urllib.request.urlopen(index_url(name), timeout=30) as r:
        for line in r.read().decode().splitlines():
            entry = json.loads(line)
            if entry["vers"] == version:
                t = entry.get("pubtime")  # absent only for very old releases
                return pkg, dt.datetime.fromisoformat(t.replace("Z", "+00:00")) if t else None
    raise LookupError(f"{name} {version} is not in the crates.io index")


def main():
    window = min_age()
    now = dt.datetime.now(dt.timezone.utc)
    pkgs = sorted(set(locked_packages()))
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        times = list(pool.map(pubtime, pkgs))
    young = [(p, t) for p, t in times if t and now - t < window]
    for (name, version), t in young:
        age = now - t
        print(f"::error::{name} {version} was published {age.days}d {age.seconds // 3600}h ago "
              f"(cooldown is {window.days} days; it is allowed from {(t + window):%Y-%m-%d %H:%M} UTC)")
    print(f"checked {len(pkgs)} crates.io packages against a {window.days}-day cooldown: "
          f"{'ok' if not young else f'{len(young)} too new'}")
    sys.exit(1 if young else 0)


if __name__ == "__main__":
    main()
