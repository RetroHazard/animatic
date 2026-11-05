#!/usr/bin/env python3
"""Release tooling for Animatic (Python 3.11+, standard library only).

Animatic uses calendar versions ``YYYY.M.PATCH`` (year >= 2000, month 1-12 without
zero padding, non-negative patch) with an optional SemVer-style ``-prerelease``
such as ``2026.11.0-rc.1``. Tags are ``v<version>``. Build metadata is rejected.

Commands (``--root`` is a global option and goes BEFORE the subcommand):

    python scripts/release.py prepare --version YYYY.M.PATCH
    python scripts/release.py validate [--tag vYYYY.M.PATCH]
    python scripts/release.py notes --version YYYY.M.PATCH --output PATH
    python scripts/release.py package --platform windows|linux --binary PATH --notices PATH --output DIR
    python scripts/release.py checksums --directory DIR

``prepare`` bumps the workspace version, every explicit internal dependency
requirement and the local crate entries of Cargo.lock in lockstep, then turns
CHANGELOG.md's Unreleased section plus the commit subjects since the latest
reachable ``vYYYY.M.PATCH`` tag into a dated release section. The new version
must belong to the current UTC year and month. It never commits, tags or pushes.
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import hashlib
import html
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from dataclasses import dataclass
from pathlib import Path

DEFAULT_ROOT = Path(__file__).resolve().parent.parent

CHANGELOG_NAME = "CHANGELOG.md"
CHECKSUMS_NAME = "SHA256SUMS"
ARCH = "x86_64"
# platform -> (archive suffix, binary name inside the archive)
PLATFORMS = {
    "windows": (".zip", "animatic.exe"),
    "linux": (".tar.gz", "animatic"),
}
# Fixed top-level files that ship in every archive; docs/*.md is added on top.
PACKAGED_FILES = ("LICENSE", "README.md", CHANGELOG_NAME)
DEP_KEYS = ("dependencies", "dev-dependencies", "build-dependencies")
# Deterministic archive metadata (the ZIP format cannot represent earlier dates).
ARCHIVE_TIME = (1980, 1, 1, 0, 0, 0)
ARCHIVE_EPOCH = 315532800  # 1980-01-01T00:00:00Z


class ReleaseError(Exception):
    """A user-facing failure; the CLI prints it and exits non-zero."""


def fail_many(headline: str, problems: list[str]) -> ReleaseError:
    return ReleaseError(headline + "".join(f"\n  - {p}" for p in problems))


# --------------------------------------------------------------------------
# Calendar versions (YYYY.M.PATCH[-prerelease]); build metadata rejected
# --------------------------------------------------------------------------

_IDENT = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
_VERSION_RE = re.compile(
    rf"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-({_IDENT}(?:\.{_IDENT})*))?"
)
MIN_YEAR, MAX_YEAR = 2000, 9999


@dataclass(frozen=True)
class Version:
    year: int
    month: int
    patch: int
    pre: tuple[str, ...] = ()

    @property
    def text(self) -> str:
        core = f"{self.year}.{self.month}.{self.patch}"
        return core + ("-" + ".".join(self.pre) if self.pre else "")

    def key(self) -> tuple:
        """Sort key: date then patch, then SemVer prerelease precedence (release > its prereleases)."""

        def ident(part: str) -> tuple:
            return (0, int(part), "") if part.isdigit() else (1, 0, part)

        return (
            self.year,
            self.month,
            self.patch,
            0 if self.pre else 1,
            tuple(ident(p) for p in self.pre),
        )


def try_parse_version(text: str) -> Version | None:
    m = _VERSION_RE.fullmatch(text)
    if not m:
        return None
    year, month, patch = int(m.group(1)), int(m.group(2)), int(m.group(3))
    if not (MIN_YEAR <= year <= MAX_YEAR and 1 <= month <= 12):
        return None
    pre = tuple(m.group(4).split(".")) if m.group(4) else ()
    return Version(year, month, patch, pre)


def parse_version(text: str, what: str = "version") -> Version:
    parsed = try_parse_version(text)
    if parsed is not None:
        return parsed
    if "+" in text:
        raise ReleaseError(f"invalid {what} {text!r}: build metadata (+...) is not supported")
    if text[:1] in ("v", "V"):
        raise ReleaseError(f"invalid {what} {text!r}: omit the leading 'v' (tags are v<version>)")
    raise ReleaseError(
        f"invalid {what} {text!r}: expected calendar version YYYY.M.PATCH (year 2000+, month 1-12 without "
        "zero padding, e.g. 2026.11.0) with an optional -prerelease"
    )


# --------------------------------------------------------------------------
# File helpers
# --------------------------------------------------------------------------


def read_text(path: Path, label: str | None = None) -> str:
    try:
        return path.read_bytes().decode("utf-8")
    except FileNotFoundError:
        raise ReleaseError(f"{label or path.name} is missing") from None
    except (OSError, UnicodeDecodeError) as exc:
        raise ReleaseError(f"cannot read {label or path.name}: {exc}") from None


def parse_toml(text: str, label: str) -> dict:
    try:
        return tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise ReleaseError(f"{label} is not valid TOML: {exc}") from None


def split_lines(text: str) -> list[str]:
    """Split keeping line endings so CRLF/LF files are rewritten faithfully."""
    return re.findall(r"[^\n]*\n|[^\n]+", text)


def _body_and_eol(line: str) -> tuple[str, str]:
    body = line.rstrip("\r\n")
    return body, line[len(body):]


def write_files_atomically(updates: dict[Path, str]) -> None:
    """Write all files or none: stage temp files, swap, roll back on failure."""
    originals: dict[Path, bytes | None] = {}
    for path in updates:
        originals[path] = path.read_bytes() if path.exists() else None
    staged: dict[Path, Path] = {}
    try:
        for path, text in updates.items():
            fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
            with os.fdopen(fd, "wb") as handle:
                handle.write(text.encode("utf-8"))
            staged[path] = Path(tmp)
            if originals[path] is not None:  # mkstemp creates 0600; keep the file's existing mode
                os.chmod(tmp, stat.S_IMODE(path.stat().st_mode))
        done: list[Path] = []
        try:
            for path, tmp in staged.items():
                os.replace(tmp, path)
                done.append(path)
        except OSError:
            for path in done:
                original = originals[path]
                if original is None:
                    path.unlink(missing_ok=True)
                else:
                    path.write_bytes(original)
            raise
    finally:
        for tmp in staged.values():
            tmp.unlink(missing_ok=True)


# --------------------------------------------------------------------------
# git
# --------------------------------------------------------------------------


def git(root: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    env.update(LC_ALL="C", GIT_TERMINAL_PROMPT="0")
    try:
        proc = subprocess.run(
            ["git", "-c", "core.quotepath=off", "-C", str(root), *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
        )
    except FileNotFoundError:
        raise ReleaseError("git executable not found") from None
    if check and proc.returncode != 0:
        raise ReleaseError(f"git {' '.join(args)} failed: {proc.stderr.strip() or proc.stdout.strip()}")
    return proc


def is_repo_root(root: Path) -> bool:
    try:
        proc = git(root, "rev-parse", "--show-toplevel", check=False)
    except ReleaseError:  # git not installed: not a repository checkout
        return False
    if proc.returncode != 0:
        return False
    try:
        return os.path.samefile(proc.stdout.strip(), root)
    except OSError:
        return False


def release_tags(tags: list[str]) -> dict[str, Version]:
    """Keep only ``v<calver>`` tags (no build metadata; old ``v1.0.0`` SemVer tags are not ours)."""
    found: dict[str, Version] = {}
    for name in tags:
        if name.startswith("v"):
            version = try_parse_version(name[1:])
            if version is not None:
                found[name] = version
    return found


def list_tags(root: Path, *, merged_into_head: bool = False) -> list[str]:
    args = ["tag", "--list"]
    if merged_into_head:
        args = ["tag", "--merged", "HEAD", "--list"]
    return [line for line in git(root, *args).stdout.splitlines() if line]


# --------------------------------------------------------------------------
# Workspace model (versions, internal dependencies, Cargo.lock)
# --------------------------------------------------------------------------


@dataclass
class Member:
    rel: str  # manifest path relative to the root, posix style
    path: Path
    name: str
    data: dict


@dataclass
class Workspace:
    root_data: dict
    version: str
    members: list[Member]

    @property
    def local_names(self) -> set[str]:
        return {m.name for m in self.members}


def load_workspace(root: Path) -> Workspace:
    """Workspace members are the explicitly listed ``[workspace] members`` (no globs)."""
    root_data = parse_toml(read_text(root / "Cargo.toml", "Cargo.toml"), "Cargo.toml")
    version = root_data.get("workspace", {}).get("package", {}).get("version")
    if not isinstance(version, str):
        raise ReleaseError("Cargo.toml has no [workspace.package] version")
    members: list[Member] = []
    for member_dir in root_data["workspace"].get("members", []):
        if any(ch in member_dir for ch in "*?["):
            raise ReleaseError(f"workspace member {member_dir!r}: list members explicitly, globs are unsupported")
        rel = f"{member_dir}/Cargo.toml"
        data = parse_toml(read_text(root / rel, rel), rel)
        name = data.get("package", {}).get("name")
        if not isinstance(name, str):
            raise ReleaseError(f"{rel} has no [package] name")
        if any(m.name == name for m in members):
            raise ReleaseError(f"duplicate workspace package name {name!r}")
        members.append(Member(rel, root / rel, name, data))
    if not members:
        raise ReleaseError("Cargo.toml lists no workspace members")
    return Workspace(root_data, version, members)


def collect_requirements(data: dict, local_names: set[str]) -> list[tuple[str, str]]:
    """Explicit version requirements on workspace-local packages, in any TOML form: (location, requirement)."""
    tables = [(key, data.get(key)) for key in DEP_KEYS]
    for target, tdata in (data.get("target") or {}).items():
        tables += [(f"target.{target}.{key}", tdata.get(key)) for key in DEP_KEYS]
    tables.append(("workspace.dependencies", (data.get("workspace") or {}).get("dependencies")))
    found: list[tuple[str, str]] = []
    for location, table in tables:
        for name, spec in (table or {}).items():
            package, requirement = name, spec if isinstance(spec, str) else None
            if isinstance(spec, dict):
                package, requirement = spec.get("package", name), spec.get("version")
            if package in local_names and isinstance(requirement, str):
                found.append((f"{location}.{name}", requirement))
    return found


# -- text-preserving edits ------------------------------------------------------
#
# Only the forms Animatic's manifests use are rewritten: the ``version`` line of
# ``[workspace.package]`` and one-line inline-table pins such as
# ``animatic-core = { version = "2026.11.0", path = "../animatic-core" }`` in any
# ``*dependencies`` table. Anything else that names a local crate with a version is
# still detected by collect_requirements, so prepare/validate fail loudly instead of
# silently leaving a stale pin.

_HEADER_RE = re.compile(r"^\s*\[+\s*([^\]]+?)\s*\]+\s*(?:#.*)?$")
_VERSION_LINE_RE = re.compile(r'^(\s*version\s*=\s*")[^"\n]*(".*)$')
_PIN_RE = re.compile(
    r'^(?P<head>\s*(?P<name>[A-Za-z0-9_-]+)\s*=\s*\{.*?(?<![\w-])version\s*=\s*")[^"\n]*(?P<tail>".*)$'
)
_LOCK_NAME_RE = re.compile(r'^name = "(?P<name>[^"]*)"$')


def rewrite_manifest(text: str, local_names: set[str], version: str) -> str:
    section = ""
    lines = split_lines(text)
    for i, line in enumerate(lines):
        body, eol = _body_and_eol(line)
        header = _HEADER_RE.match(body)
        if header:
            section = header.group(1)
        elif section == "workspace.package":
            m = _VERSION_LINE_RE.match(body)
            if m:
                lines[i] = f"{m.group(1)}{version}{m.group(2)}{eol}"
        elif section.endswith("dependencies"):
            m = _PIN_RE.match(body)
            if m and m.group("name") in local_names:
                lines[i] = f'{m.group("head")}{version}{m.group("tail")}{eol}'
    return "".join(lines)


def rewrite_lock(text: str, local_names: set[str], version: str) -> str:
    """Set the version of workspace-local ``[[package]]`` entries (those without a ``source``)."""
    lines = split_lines(text)
    starts = [i for i, line in enumerate(lines) if _body_and_eol(line)[0].strip() == "[[package]]"]
    for start, end in zip(starts, starts[1:] + [len(lines)]):
        bodies = [_body_and_eol(line)[0] for line in lines[start:end]]
        name = next((m.group("name") for b in bodies if (m := _LOCK_NAME_RE.match(b))), None)
        if name not in local_names or any(b.startswith("source = ") for b in bodies):
            continue
        for offset, body in enumerate(bodies):
            if body.startswith("version = "):
                lines[start + offset] = f'version = "{version}"{_body_and_eol(lines[start + offset])[1]}'
                break
    return "".join(lines)


def lock_problems(lock_data: dict, local_names: set[str], version: str) -> list[str]:
    found: dict[str, list[str]] = {}
    for pkg in lock_data.get("package", []):
        if pkg.get("name") in local_names and "source" not in pkg:
            found.setdefault(pkg["name"], []).append(str(pkg.get("version")))
    problems: list[str] = []
    for name in sorted(local_names):
        if name not in found:
            problems.append(f"Cargo.lock has no entry for local package {name}; run cargo to refresh it")
        problems.extend(f"Cargo.lock: {name} is {v} (expected {version})" for v in found.get(name, []) if v != version)
    return problems


def workspace_problems(ws: Workspace, lock_data: dict | None) -> list[str]:
    """Every consistency problem across manifests and Cargo.lock for the workspace version."""
    problems: list[str] = []
    if try_parse_version(ws.version) is None:
        problems.append(f"workspace version {ws.version!r} is not a calendar version (YYYY.M.PATCH[-pre])")
    for member in [*ws.members, Member("Cargo.toml", Path("Cargo.toml"), "", ws.root_data)]:
        if member.name and member.data.get("package", {}).get("version") != {"workspace": True}:
            problems.append(f"{member.rel}: package version must be 'version.workspace = true'")
        for location, requirement in collect_requirements(member.data, ws.local_names):
            if requirement != ws.version:
                problems.append(f"{member.rel}: {location} requires {requirement!r} (expected {ws.version!r})")
    if lock_data is None:
        problems.append("Cargo.lock is missing")
    else:
        problems.extend(lock_problems(lock_data, ws.local_names, ws.version))
    return problems


def read_lock(root: Path) -> dict:
    return parse_toml(read_text(root / "Cargo.lock", "Cargo.lock"), "Cargo.lock")


# --------------------------------------------------------------------------
# CHANGELOG.md model
# --------------------------------------------------------------------------

_SECTION_RE = re.compile(r"^##\s+(?P<title>\S+)(?:\s+-\s+\S+)?\s*$")
_GROUP_RE = re.compile(r"^###\s+(?P<title>.+?)\s*$")


@dataclass
class Section:
    heading: str
    title: str | None  # "Unreleased" or a calendar version; None for any other heading
    lines: list[str]  # body lines exactly as written (no trailing newline characters)

    @property
    def is_unreleased(self) -> bool:
        return self.title is not None and self.title.lower() == "unreleased"

    @property
    def version(self) -> Version | None:
        return try_parse_version(self.title) if self.title else None


@dataclass
class Changelog:
    header: list[str]
    sections: list[Section]
    eol: str

    def released(self) -> list[Section]:
        return [s for s in self.sections if s.version is not None]

    def unreleased(self) -> Section | None:
        for section in self.sections:
            if section.is_unreleased:
                return section
        return None

    def find(self, version: str) -> list[Section]:
        return [s for s in self.sections if s.title == version and s.version is not None]


def parse_changelog(text: str) -> Changelog:
    eol = "\r\n" if "\r\n" in text else "\n"
    lines = text.replace("\r\n", "\n").split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    header: list[str] = []
    sections: list[Section] = []
    fence: str | None = None
    for line in lines:
        stripped = line.lstrip()
        if stripped.startswith(("```", "~~~")):
            marker = stripped[:3]
            fence = None if fence == marker else (fence or marker)
        if line.startswith("## ") and not fence:
            m = _SECTION_RE.match(line)
            title = m.group("title") if m else None
            if title is not None and not (title.lower() == "unreleased" or try_parse_version(title)):
                title = None
            sections.append(Section(line, title, []))
        elif sections:
            sections[-1].lines.append(line)
        else:
            header.append(line)
    return Changelog(header, sections, eol)


def load_changelog(root: Path) -> Changelog:
    return parse_changelog(read_text(root / CHANGELOG_NAME, CHANGELOG_NAME))


def trim_blank(lines: list[str]) -> list[str]:
    start, end = 0, len(lines)
    while start < end and not lines[start].strip():
        start += 1
    while end > start and not lines[end - 1].strip():
        end -= 1
    return lines[start:end]


def has_content(section: Section) -> bool:
    return any(line.strip() and not line.lstrip().startswith("#") for line in section.lines)


def changelog_problems(log: Changelog) -> list[str]:
    problems: list[str] = []
    seen: dict[str, int] = {}
    unreleased = 0
    for section in log.sections:
        if section.title is None:
            if re.match(r"^##\s+\[?v?\d", section.heading):
                problems.append(f"{CHANGELOG_NAME}: invalid version heading {section.heading!r}")
        elif section.is_unreleased:
            unreleased += 1
        else:
            seen[section.title] = seen.get(section.title, 0) + 1
    problems.extend(f"{CHANGELOG_NAME}: duplicate section for {v}" for v, n in seen.items() if n > 1)
    if unreleased > 1:
        problems.append(f"{CHANGELOG_NAME}: more than one Unreleased section")
    return problems


def split_groups(body: list[str]) -> tuple[list[str], list[list]]:
    """Split an Unreleased body into free-form preamble lines and ``### Heading`` groups.

    Groups are ``[heading, lines]``; repeated headings (case-insensitive) are merged.
    """
    preamble: list[str] = []
    groups: list[list] = []
    current: list | None = None
    for line in body:
        m = _GROUP_RE.match(line)
        if m:
            title = m.group("title")
            current = next((g for g in groups if g[0].lower() == title.lower()), None)
            if current is None:
                current = [title, []]
                groups.append(current)
        elif current is not None:
            current[1].append(line)
        else:
            preamble.append(line)
    return trim_blank(preamble), groups


# --------------------------------------------------------------------------
# Commit-derived changelog entries
# --------------------------------------------------------------------------

TYPE_HEADINGS = {
    "feat": "Added",
    "fix": "Fixed",
    "perf": "Performance",
    "refactor": "Changed",
    "revert": "Reverted",
    "docs": "Documentation",
    "test": "Tests",
    "tests": "Tests",
    "build": "Build and CI",
    "ci": "Build and CI",
    "chore": "Maintenance",
    "style": "Maintenance",
}
BREAKING_HEADING = "Breaking Changes"
OTHER_HEADING = "Other changes"
HEADING_ORDER = [
    BREAKING_HEADING,
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
    "Performance",
    "Reverted",
    "Documentation",
    "Tests",
    "Build and CI",
    "Maintenance",
]

_CONVENTIONAL_RE = re.compile(
    r"^(?P<type>[A-Za-z]+)(?:\((?P<scope>[^()\r\n]*)\))?(?P<bang>!)?:\s+(?P<desc>\S.*)$"
)
_BREAKING_BODY_RE = re.compile(r"(?m)^BREAKING[ -]CHANGE:")
_NOISE_RES = [
    re.compile(r"^Merge\b"),
    re.compile(r"^(?:fixup|squash|amend)!"),
    re.compile(r"^(?:chore|build|ci)\(release\)!?:", re.I),
    re.compile(
        r"^(?:chore|build|ci)(?:\([^)]*\))?!?:\s*(?:release|prepare(?: for)? release|bump(?: the)? version|version bump)\b",
        re.I,
    ),
    re.compile(r"^(?:release|prepare(?: for)? release|bump version)\s*(?:to\s+)?v?\d+\.\d+\.\d+", re.I),
]


@dataclass(frozen=True)
class Commit:
    sha: str
    subject: str
    body: str


def heading_rank(heading: str) -> int:
    lowered = [h.lower() for h in HEADING_ORDER]
    if heading.lower() in lowered:
        return lowered.index(heading.lower())
    if heading.lower() == OTHER_HEADING.lower():
        return len(HEADING_ORDER) + 100
    return len(HEADING_ORDER)


def collect_commits(root: Path, since_tag: str | None) -> list[Commit]:
    rev = f"refs/tags/{since_tag}..HEAD" if since_tag else "HEAD"
    out = git(root, "log", "--no-merges", "--reverse", "--format=%H%x1f%s%x1f%b%x1e", rev).stdout
    commits: list[Commit] = []
    for record in out.split("\x1e"):
        record = record.strip("\n")
        if not record.strip():
            continue
        sha, subject, body = record.split("\x1f", 2)
        commits.append(Commit(sha, subject.strip(), body.strip()))
    return commits


def is_noise(commit: Commit) -> bool:
    return any(rx.match(commit.subject) for rx in _NOISE_RES)


def normalize_entry(text: str) -> str:
    text = re.sub(r"^\s*[-*+]\s+", "", text)
    text = re.sub(r"\s*\([0-9a-f]{7,40}\)\s*$", "", text)
    text = text.replace("**", "")
    return re.sub(r"\s+", " ", text).strip().lower()


_MARKDOWN_SPECIALS = re.compile(r"([\\`*_\[\]()!|~#])")


def md_escape(text: str) -> str:
    """Make commit-derived text inert in Markdown and raw HTML (GitHub renders both)."""
    return _MARKDOWN_SPECIALS.sub(r"\\\1", html.escape(text, quote=False))


def categorize(commit: Commit) -> tuple[str, str, set[str]]:
    """Return (heading, markdown bullet, normalized texts used for duplicate detection)."""
    short = commit.sha[:7]
    m = _CONVENTIONAL_RE.match(commit.subject)
    if m and m.group("type").lower() in TYPE_HEADINGS:
        scope = (m.group("scope") or "").strip()
        desc = m.group("desc").strip()
        breaking = bool(m.group("bang")) or bool(_BREAKING_BODY_RE.search(commit.body))
        heading = BREAKING_HEADING if breaking else TYPE_HEADINGS[m.group("type").lower()]
        prefix = f"**{md_escape(scope)}:** " if scope else ""
        bullet = f"- {prefix}{md_escape(desc)} ({short})"
        keys = {normalize_entry(commit.subject), normalize_entry(desc)}
        if scope:
            keys.add(normalize_entry(f"{scope}: {desc}"))
        return heading, bullet, keys
    breaking = bool(_BREAKING_BODY_RE.search(commit.body))
    heading = BREAKING_HEADING if breaking else OTHER_HEADING
    return heading, f"- {md_escape(commit.subject)} ({short})", {normalize_entry(commit.subject)}


def render_release_section(
    version: str, date: str, unreleased: Section | None, commits: list[Commit]
) -> list[str]:
    """Curated Unreleased text first, then generated bullets, under merged headings."""
    preamble: list[str] = []
    groups: list[list] = []  # [heading, lines]
    if unreleased is not None:
        preamble, groups = split_groups(unreleased.lines)
    curated_texts = {
        normalize_entry(line)
        for _, lines in groups
        for line in lines
        if re.match(r"^\s*[-*+]\s+", line)
    }
    seen = set(curated_texts)
    for commit in commits:
        if is_noise(commit):
            continue
        heading, bullet, keys = categorize(commit)
        if keys & seen:
            continue
        seen |= keys
        group = next((g for g in groups if g[0].lower() == heading.lower()), None)
        if group is None:
            group = [heading, []]
            groups.append(group)
        group[1][:] = trim_blank(group[1]) + [bullet]
    ordered = sorted(
        ((h, trim_blank(lines)) for h, lines in groups if trim_blank(lines)),
        key=lambda g: heading_rank(g[0]),
    )
    body: list[str] = []
    if preamble:
        body += preamble + [""]
    for heading, lines in ordered:
        body += [f"### {heading}", ""] + lines + [""]
    body = trim_blank(body)
    if not body:
        return []
    return [f"## {version} - {date}", ""] + body


def build_changelog_text(
    log: Changelog, release_section: list[str]
) -> str:
    out: list[str] = trim_blank(log.header) + [""]
    out += ["## Unreleased", ""]
    out += release_section + [""]
    for section in log.sections:
        if section.is_unreleased:
            continue
        out.append(section.heading)
        out += section.lines
    out = trim_blank_tail(out)
    return log.eol.join(out) + log.eol


def trim_blank_tail(lines: list[str]) -> list[str]:
    end = len(lines)
    while end > 0 and not lines[end - 1].strip():
        end -= 1
    return lines[:end]


# --------------------------------------------------------------------------
# Repository validation
# --------------------------------------------------------------------------


@dataclass
class RepoState:
    workspace: Workspace
    version: Version
    changelog: Changelog


def check_repository(root: Path, tag: str | None = None) -> RepoState:
    """Raise ReleaseError listing every inconsistency; return the parsed state otherwise."""
    problems: list[str] = []
    ws = load_workspace(root)
    lock_data = read_lock(root) if (root / "Cargo.lock").exists() else None
    problems += workspace_problems(ws, lock_data)
    version = try_parse_version(ws.version)
    log = load_changelog(root)
    problems += changelog_problems(log)
    if version is not None:
        released = log.released()
        if released:
            highest = max((s.version for s in released if s.version), key=Version.key)
            if highest.text != version.text:
                problems.append(
                    f"workspace version {version.text} differs from the latest changelog release {highest.text}"
                )
        if tag is not None:
            tag_version = None
            if tag.startswith("v"):
                tag_version = try_parse_version(tag[1:])
            if tag_version is None:
                problems.append(f"tag {tag!r} is not of the form v<MAJOR.MINOR.PATCH[-prerelease]>")
            elif tag_version.text != version.text:
                problems.append(f"tag {tag} does not match workspace version {version.text}")
            else:
                matches = log.find(version.text)
                if not matches:
                    problems.append(f"{CHANGELOG_NAME} has no section for {version.text}")
                elif not any(has_content(s) for s in matches):
                    problems.append(f"{CHANGELOG_NAME} section for {version.text} is empty")
    if problems:
        raise fail_many("release metadata is inconsistent:", problems)
    assert version is not None
    return RepoState(ws, version, log)


# --------------------------------------------------------------------------
# prepare
# --------------------------------------------------------------------------


@dataclass
class PrepareResult:
    version: str
    previous_tag: str | None
    commit_count: int
    changed: list[str]


def utc_today() -> dt.date:
    return dt.datetime.now(dt.timezone.utc).date()


def prepare(root: Path, version_text: str, *, today: dt.date | None = None) -> PrepareResult:
    """Prepare a release. ``today`` exists only so tests can pin the UTC date; the CLI never sets it."""
    new = parse_version(version_text)
    if today is None:
        today = utc_today()
    if (new.year, new.month) != (today.year, today.month):
        kind = "future-dated" if (new.year, new.month) > (today.year, today.month) else "backdated"
        raise ReleaseError(
            f"{new.text} is {kind}: new releases must use the current UTC year and month "
            f"({today.year}.{today.month}.PATCH)"
        )
    date = today.isoformat()

    ws = load_workspace(root)
    current = parse_version(ws.version, "workspace version")
    log = load_changelog(root)
    problems = changelog_problems(log)
    if problems:
        raise fail_many("cannot prepare a release:", problems)
    if not is_repo_root(root):
        raise ReleaseError("prepare needs the repository root to be a git checkout")
    shallow = git(root, "rev-parse", "--is-shallow-repository").stdout.strip()
    if shallow == "true":
        raise ReleaseError(
            "this is a shallow clone, so tags and history are incomplete; "
            "run 'git fetch --unshallow --tags' first"
        )

    if log.find(new.text):
        raise ReleaseError(f"{CHANGELOG_NAME} already has a section for {new.text}")
    all_tags = release_tags(list_tags(root))
    tag_name = f"v{new.text}"
    if tag_name in set(list_tags(root)):
        raise ReleaseError(f"tag {tag_name} already exists")
    reachable = release_tags(list_tags(root, merged_into_head=True))

    released_versions = [s.version for s in log.released() if s.version]
    first_release = not released_versions and not all_tags
    known = [current, *released_versions, *reachable.values()]
    if first_release:
        if new.key() < current.key():
            raise ReleaseError(f"{new.text} is lower than the workspace version {current.text}")
    else:
        latest = max(known, key=Version.key)
        if new.key() <= latest.key():
            verb = "equal to" if new.key() == latest.key() else "lower than"
            raise ReleaseError(f"{new.text} is {verb} the latest version {latest.text}; versions must increase")

    previous_tag = max(reachable, key=lambda name: reachable[name].key()) if reachable else None
    commits = collect_commits(root, previous_tag)
    unreleased = log.unreleased()
    section = render_release_section(new.text, date, unreleased, commits)
    if not section:
        scope = f"since {previous_tag}" if previous_tag else "in the repository history"
        raise ReleaseError(f"nothing to release: no curated Unreleased entries or releasable commits {scope}")
    used = [c for c in commits if not is_noise(c)]

    updates: dict[Path, str] = {
        member.path: rewrite_manifest(read_text(member.path, member.rel), ws.local_names, new.text)
        for member in ws.members
    }
    root_manifest = root / "Cargo.toml"
    updates[root_manifest] = rewrite_manifest(read_text(root_manifest, "Cargo.toml"), ws.local_names, new.text)
    lock_path = root / "Cargo.lock"
    updates[lock_path] = rewrite_lock(read_text(lock_path, "Cargo.lock"), ws.local_names, new.text)
    updates[root / CHANGELOG_NAME] = build_changelog_text(log, section)

    # Verify the planned result in memory (re-parsed from the rewritten text) before touching the disk.
    planned_root = parse_toml(updates[root_manifest], "Cargo.toml")
    planned = Workspace(
        planned_root,
        planned_root["workspace"]["package"]["version"],
        [Member(m.rel, m.path, m.name, parse_toml(updates[m.path], m.rel)) for m in ws.members],
    )
    issues = workspace_problems(planned, parse_toml(updates[lock_path], "Cargo.lock"))
    if planned.version != new.text:
        issues.append("[workspace.package] version was not updated")
    if issues:
        raise fail_many("cannot update version metadata:", issues)

    changed: list[str] = []
    to_write: dict[Path, str] = {}
    for path, text in updates.items():
        existing = path.read_bytes().decode("utf-8") if path.exists() else None
        if existing != text:
            to_write[path] = text
            changed.append(path.relative_to(root).as_posix())
    changed.sort()
    originals = {path: path.read_bytes() for path in to_write if path.exists()}
    write_files_atomically(to_write)
    try:
        check_repository(root, f"v{new.text}")
    except ReleaseError as exc:
        for path in to_write:  # never leave a half-valid release state behind
            if path in originals:
                path.write_bytes(originals[path])
            else:
                path.unlink(missing_ok=True)
        raise ReleaseError(f"post-write validation failed, changes reverted: {exc}") from None
    return PrepareResult(new.text, previous_tag, len(used), changed)


# --------------------------------------------------------------------------
# notes
# --------------------------------------------------------------------------


def extract_notes(root: Path, version_text: str) -> str:
    version = parse_version(version_text)
    log = load_changelog(root)
    matches = log.find(version.text)
    if not matches:
        raise ReleaseError(f"{CHANGELOG_NAME} has no section for {version.text}")
    if len(matches) > 1:
        raise ReleaseError(f"{CHANGELOG_NAME} has duplicate sections for {version.text}")
    section = matches[0]
    if not has_content(section):
        raise ReleaseError(f"{CHANGELOG_NAME} section for {version.text} is empty")
    return "\n".join(trim_blank(section.lines)) + "\n"


# --------------------------------------------------------------------------
# package
# --------------------------------------------------------------------------


def archive_stem(version: str, platform: str) -> str:
    return f"animatic-v{version}-{platform}-{ARCH}"


def archive_name(version: str, platform: str) -> str:
    return archive_stem(version, platform) + PLATFORMS[platform][0]


def _require_regular(root: Path, name: str) -> None:
    """A packaged repository file must be a regular, non-symlink file that really lives inside the
    repository (a symlinked or junctioned parent directory could otherwise point anywhere)."""
    path = root / name
    if path.is_symlink() or not path.is_file():
        raise ReleaseError(f"{name} must be a regular file")
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError:
        raise ReleaseError(f"{name} resolves outside the repository") from None


def doc_files(root: Path) -> list[str]:
    """The tracked ``docs/*.md`` files. Only git-tracked files are eligible, so untracked or
    ignored local drafts can never ship; a tracked file missing from the working tree is an error."""
    if not is_repo_root(root):
        raise ReleaseError("package needs the repository root to be a git checkout (docs are taken from git)")
    docs_dir = root / "docs"
    if docs_dir.is_symlink() or (docs_dir.exists() and not docs_dir.resolve().is_relative_to(root.resolve())):
        raise ReleaseError("docs/ is a link or resolves outside the repository")
    out = git(root, "ls-files", "-z", "--", "docs").stdout
    names = sorted({e for e in out.split("\0") if re.fullmatch(r"docs/[^/]+\.md", e)})
    if not names:
        raise ReleaseError("no tracked docs/*.md files to package")
    for name in names:
        if not (root / name).exists() and not (root / name).is_symlink():
            raise ReleaseError(f"tracked file {name} is missing from the working tree")
        _require_regular(root, name)
    return names


def package_files(root: Path) -> list[str]:
    for name in PACKAGED_FILES:
        if not (root / name).exists():
            raise ReleaseError(f"{name} is missing")
        _require_regular(root, name)
    return [*PACKAGED_FILES, *doc_files(root)]


def _copy_to_zip(zf: zipfile.ZipFile, arcname: str, source: Path, mode: int) -> None:
    info = zipfile.ZipInfo(arcname, ARCHIVE_TIME)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.create_system = 3
    info.external_attr = (0o100000 | mode) << 16
    with source.open("rb") as src, zf.open(info, "w") as dst:
        shutil.copyfileobj(src, dst, 1024 * 1024)


def _add_to_tar(tf: tarfile.TarFile, arcname: str, source: Path | None, mode: int) -> None:
    info = tarfile.TarInfo(arcname)
    info.mtime = ARCHIVE_EPOCH
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mode = mode
    if source is None:
        info.type = tarfile.DIRTYPE
        tf.addfile(info)
        return
    info.type = tarfile.REGTYPE
    info.size = source.stat().st_size
    with source.open("rb") as src:
        tf.addfile(info, src)


NOTICES_NAME = "THIRD-PARTY-NOTICES.html"


def build_package(root: Path, platform: str, binary: Path, notices: Path, output: Path) -> Path:
    if platform not in PLATFORMS:
        raise ReleaseError(f"unsupported platform {platform!r}; expected one of {', '.join(PLATFORMS)}")
    state = check_repository(root)
    if binary.is_symlink():
        raise ReleaseError(f"binary {binary} must be a regular file, not a symbolic link")
    if not binary.exists():
        raise ReleaseError(f"binary {binary} does not exist")
    if not binary.is_file():
        raise ReleaseError(f"binary {binary} must be a file, not a directory")
    if binary.stat().st_size == 0:
        raise ReleaseError(f"binary {binary} is empty")
    if notices.is_symlink():
        raise ReleaseError(f"notices file {notices} must be a regular file, not a symbolic link")
    if not notices.exists():
        raise ReleaseError(f"notices file {notices} does not exist")
    if not notices.is_file():
        raise ReleaseError(f"notices file {notices} must be a regular file")
    if notices.stat().st_size == 0:
        raise ReleaseError(f"notices file {notices} is empty")
    if output.exists() and not output.is_dir():
        raise ReleaseError(f"output {output} exists and is not a directory")

    files = package_files(root)
    stem = archive_stem(state.version.text, platform)
    binary_name = PLATFORMS[platform][1]
    entries: list[tuple[str, Path, int]] = [(binary_name, binary, 0o755)]
    entries += [(name, root / name, 0o644) for name in PACKAGED_FILES]
    entries.append((NOTICES_NAME, notices, 0o644))
    entries += [(name, root / name, 0o644) for name in files if name not in PACKAGED_FILES]

    output.mkdir(parents=True, exist_ok=True)
    final = output / archive_name(state.version.text, platform)
    fd, tmp_name = tempfile.mkstemp(dir=output, prefix=".animatic-", suffix=".tmp")
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "wb") as raw:
            if platform == "windows":
                with zipfile.ZipFile(raw, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
                    for name, source, mode in entries:
                        _copy_to_zip(zf, f"{stem}/{name}", source, mode)
            else:
                with gzip.GzipFile(filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=0) as gz:
                    with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tf:
                        _add_to_tar(tf, stem, None, 0o755)
                        docs_dir_added = False
                        for name, source, mode in entries:
                            if name.startswith("docs/") and not docs_dir_added:
                                _add_to_tar(tf, f"{stem}/docs", None, 0o755)
                                docs_dir_added = True
                            _add_to_tar(tf, f"{stem}/{name}", source, mode)
        os.replace(tmp, final)
    finally:
        tmp.unlink(missing_ok=True)
    return final


# --------------------------------------------------------------------------
# checksums
# --------------------------------------------------------------------------


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_checksums(root: Path, directory: Path) -> Path:
    state = check_repository(root)
    if not directory.is_dir():
        raise ReleaseError(f"{directory} is not a directory")
    expected = {archive_name(state.version.text, p) for p in PLATFORMS}
    problems: list[str] = []
    seen: set[str] = set()
    with os.scandir(directory) as scan:
        entries = sorted(scan, key=lambda e: e.name)
    for entry in entries:
        seen.add(entry.name)
        if entry.is_symlink():
            problems.append(f"{entry.name} is a symbolic link")
        elif entry.name == CHECKSUMS_NAME:
            if not entry.is_file(follow_symlinks=False):
                problems.append(f"{entry.name} is not a regular file")
        elif entry.name in expected:
            if not entry.is_file(follow_symlinks=False):
                problems.append(f"{entry.name} is not a regular file")
            elif entry.stat().st_size == 0:
                problems.append(f"{entry.name} is empty")
        else:
            problems.append(f"unexpected entry {entry.name}")
    problems.extend(f"missing archive {name}" for name in sorted(expected - seen))
    if problems:
        raise fail_many(f"cannot write {CHECKSUMS_NAME}:", problems)
    lines = [f"{sha256_file(directory / name)}  {name}\n" for name in sorted(expected)]
    fd, tmp_name = tempfile.mkstemp(dir=directory, prefix=".sha256sums.", suffix=".tmp")
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write("".join(lines).encode("ascii"))
        os.replace(tmp, directory / CHECKSUMS_NAME)
    finally:
        tmp.unlink(missing_ok=True)
    return directory / CHECKSUMS_NAME


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="release.py", description="Animatic release tooling.")
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="repository root (default: inferred from this script)")
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("prepare", help="bump versions and generate the changelog release section")
    p.add_argument("--version", required=True, help="new calendar version, e.g. 2026.11.0 or 2026.11.1-rc.1")

    p = sub.add_parser("validate", help="check version/changelog/lock consistency")
    p.add_argument("--tag", help="release tag (vVERSION) that must match the workspace version")

    p = sub.add_parser("notes", help="write one release's changelog section for GitHub release notes")
    p.add_argument("--version", required=True)
    p.add_argument("--output", required=True, type=Path)

    p = sub.add_parser("package", help="build a release archive")
    p.add_argument("--platform", required=True, choices=sorted(PLATFORMS))
    p.add_argument("--binary", required=True, type=Path)
    p.add_argument("--notices", required=True, type=Path, help="generated third-party license notices (HTML)")
    p.add_argument("--output", required=True, type=Path, help="output directory")

    p = sub.add_parser("checksums", help="write SHA256SUMS for the release archives")
    p.add_argument("--directory", required=True, type=Path)
    return parser


def run(args: argparse.Namespace) -> None:
    root = args.root.resolve()
    if not (root / "Cargo.toml").is_file():
        raise ReleaseError(f"{root} is not a Cargo workspace root (Cargo.toml not found)")
    if args.command == "prepare":
        result = prepare(root, args.version)
        since = f"since {result.previous_tag}" if result.previous_tag else "from the repository history"
        print(f"Prepared release {result.version}: {result.commit_count} commit(s) {since}")
        for path in result.changed:
            print(f"  updated {path}")
    elif args.command == "validate":
        state = check_repository(root, args.tag)
        suffix = f" (tag {args.tag})" if args.tag else ""
        print(f"OK: release metadata is consistent at {state.version.text}{suffix}")
    elif args.command == "notes":
        text = extract_notes(root, args.version)
        if args.output.is_dir():
            raise ReleaseError(f"output {args.output} is a directory")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(text.encode("utf-8"))
        print(f"Wrote release notes for {args.version} to {args.output}")
    elif args.command == "package":
        print(build_package(root, args.platform, args.binary, args.notices, args.output))
    elif args.command == "checksums":
        path = write_checksums(root, args.directory)
        print(path.read_text(encoding="ascii"), end="")


def main(argv: list[str] | None = None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")  # type: ignore[union-attr]
        except (AttributeError, ValueError):
            pass
    args = build_parser().parse_args(argv)
    try:
        run(args)
    except ReleaseError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
