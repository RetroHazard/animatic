#!/usr/bin/env python3
"""Continuous-integration helpers for Animatic (Python 3.11+, standard library only).

Commands (``--root`` is a global option and goes BEFORE the subcommand):

    python scripts/ci.py plan --base SHA [--head SHA] [--force] [--github-output FILE]
    python scripts/ci.py docs
    python scripts/ci.py gate

``plan`` decides which expensive CI jobs a change needs. It diffs ``--base`` against
``--head`` (default ``HEAD``) with ``git diff --no-renames --name-only -z`` so deleted
paths and both sides of a rename are classified, prints ``{"rust": bool, "licenses":
bool}`` on stdout and, with ``--github-output``, appends ``rust=true|false`` and
``licenses=true|false`` lines to that file. ``--force`` selects everything without
touching git (``--base``/``--head`` may then be missing). An all-zero base (a push
that created the branch) also selects everything. Without ``--force`` a missing,
empty or unavailable base is an error: the planner never assumes that a change can
be skipped. Anything it cannot positively classify as
cheap selects both Rust and licence checks (fail closed). The always-on quality job is
not planned; it runs for every change.

``docs`` validates every tracked Markdown file of the checkout: relative file links
and images, ``#anchor`` fragments (GitHub heading slugs, duplicate suffixes, explicit
HTML ids), balanced code fences and consistent table column counts. Fenced code,
inline code, HTML comments and external URLs are not inspected. Targets must be
tracked paths of the source checkout (release archives are not considered).

``gate`` is the ``Required checks`` job logic. It reads ``CI_NEEDS_JSON`` (the
``toJSON(needs)`` of the gate job) plus the ``CI_RUST``, ``CI_LICENSES`` and
``CI_PACKAGE`` selection strings (exactly ``true`` or ``false``) and demands that
plan and quality succeeded and that every other job succeeded if it was selected
and was *skipped* if it was not. Failed, cancelled, missing or unexpectedly
skipped/run jobs fail the gate with a concrete reason.
"""

from __future__ import annotations

import argparse
import html
import json
import os
import posixpath
import re
import subprocess
import sys
import unicodedata
from collections.abc import Collection, Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import unquote

DEFAULT_ROOT = Path(__file__).resolve().parent.parent

# job id -> selection flag env name (None: must always succeed). Order = report order.
GATE_JOBS = (
    ("plan", None),
    ("quality", None),
    ("notices", "CI_LICENSES"),
    ("rust", "CI_RUST"),
    ("package", "CI_PACKAGE"),
)


class CiError(Exception):
    """A user-facing failure; the CLI prints it and exits non-zero."""


# --------------------------------------------------------------------------- git


def git(root: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if k not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
    env.update(LC_ALL="C", GIT_TERMINAL_PROMPT="0")
    try:
        proc = subprocess.run(
            ["git", "-c", "core.quotepath=off", "-C", str(root), *args],
            capture_output=True,
            env=env,
        )
    except OSError as exc:
        raise CiError(f"cannot run git: {exc}") from exc
    if proc.returncode != 0:
        detail = proc.stderr.decode("utf-8", "replace").strip() or f"exit status {proc.returncode}"
        raise CiError(f"git {' '.join(args[:2])} failed: {detail}")
    return proc.stdout.decode("utf-8", "surrogateescape")


# -------------------------------------------------------------------------- plan

BOTH = (True, True)
RUST = (True, False)
LICENSES = (False, True)
NEITHER = (False, False)

MARKDOWN_SUFFIXES = (".md", ".markdown")
DOC_ASSET_SUFFIXES = (".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp")
# Packaging scripts: their change can alter release archives, so everything runs.
PACKAGING_SCRIPTS = {"release.py", "test_release.py"}
RUST_TOOL_CONFIGS = {"rustfmt.toml", ".rustfmt.toml", "clippy.toml", ".clippy.toml"}


def classify_path(path: str) -> tuple[bool, bool]:
    """Return ``(rust, licenses)`` for one repository-relative POSIX path.

    Unknown paths select everything: a new kind of file must be classified here on
    purpose before it may skip the expensive jobs.
    """
    parts = path.split("/")
    name = parts[-1]
    top = parts[0]
    suffix = posixpath.splitext(name)[1].lower()
    nested = len(parts) > 1

    # Build, toolchain and automation inputs affect everything, including packaging.
    if top in (".github", ".cargo") or name.startswith("rust-toolchain"):
        return BOTH
    if name in ("Cargo.toml", "Cargo.lock"):
        return BOTH
    if not nested and name in ("about.toml", "about.hbs", "LICENSE"):
        return LICENSES
    if top == "crates":
        return RUST
    if suffix in MARKDOWN_SUFFIXES:
        return NEITHER
    if top == "docs":
        return NEITHER if suffix in DOC_ASSET_SUFFIXES else BOTH
    if top == "scripts":
        if len(parts) == 2 and suffix == ".py" and name not in PACKAGING_SCRIPTS:
            return NEITHER  # other Python tooling only affects the always-on quality job
        return BOTH  # release.py, test_release.py and anything unrecognised
    if name in RUST_TOOL_CONFIGS or (not nested and suffix == ".rs"):
        return RUST
    return BOTH


def select_paths(paths: Iterable[str]) -> tuple[bool, bool]:
    """OR-aggregate :func:`classify_path`; no changed path selects nothing."""
    rust = licenses = False
    for path in paths:
        r, lic = classify_path(path)
        rust |= r
        licenses |= lic
    return rust, licenses


def _is_zero_sha(value: str) -> bool:
    return bool(value) and set(value) == {"0"}


def _resolve_commit(root: Path, label: str, ref: str) -> str:
    if ref.startswith("-"):
        raise CiError(f"{label} {ref!r} is not a valid revision")
    try:
        return git(root, "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}").strip()
    except CiError as exc:
        raise CiError(
            f"{label} commit {ref!r} is unavailable in {root} (shallow checkout? use fetch-depth: 0 "
            "or pass --force to run every check)"
        ) from exc


def changed_paths(root: Path, base: str, head: str) -> list[str]:
    base_sha = _resolve_commit(root, "base", base)
    head_sha = _resolve_commit(root, "head", head)
    out = git(root, "diff", "--no-renames", "--name-only", "-z", "--no-ext-diff", base_sha, head_sha, "--")
    return [p for p in out.split("\0") if p]


def plan(root: Path, base: str | None, head: str | None, force: bool) -> tuple[bool, bool, list[str]]:
    """Return ``(rust, licenses, changed paths)``; ``force`` never consults git."""
    if force:
        return True, True, []
    if base is None or not base.strip():
        raise CiError("--base is required unless --force is given")
    if _is_zero_sha(base.strip()):
        return True, True, []  # the push created the branch: there is nothing to diff against
    paths = changed_paths(root, base.strip(), (head or "HEAD").strip() or "HEAD")
    rust, licenses = select_paths(paths)
    return rust, licenses, paths


def _bool(value: bool) -> str:
    return "true" if value else "false"


def write_github_output(path: Path, rust: bool, licenses: bool) -> None:
    try:
        with open(path, "a", encoding="utf-8", newline="\n") as handle:
            handle.write(f"rust={_bool(rust)}\nlicenses={_bool(licenses)}\n")
    except OSError as exc:
        raise CiError(f"cannot write {path}: {exc}") from exc


def run_plan(args: argparse.Namespace) -> int:
    rust, licenses, paths = plan(args.root.resolve(), args.base, args.head, args.force)
    if args.force or _is_zero_sha((args.base or "").strip()):
        print("plan: forced or no base commit; every check selected", file=sys.stderr)
    else:
        print(f"plan: {len(paths)} changed path(s)", file=sys.stderr)
        for path in paths[:100]:
            r, lic = classify_path(path)
            label = "rust+licenses" if r and lic else "rust" if r else "licenses" if lic else "quality only"
            print(f"  {label:13} {path}", file=sys.stderr)
        if len(paths) > 100:
            print(f"  ... and {len(paths) - 100} more", file=sys.stderr)
    print(f"plan: rust={_bool(rust)} licenses={_bool(licenses)}", file=sys.stderr)
    print(json.dumps({"rust": rust, "licenses": licenses}))
    if args.github_output is not None:
        write_github_output(args.github_output, rust, licenses)
    return 0


# -------------------------------------------------------------------------- docs

_FENCE = re.compile(r"^(?:\s{0,3}>)*\s*(`{3,}|~{3,})(.*)$")
_QUOTE_PREFIX = re.compile(r"^(?:\s{0,3}>\s?)+")
_ATX_HEADING = re.compile(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$")
_ATX_CLOSER = re.compile(r"(?:^|[ \t]+)#+[ \t]*$")
_SETEXT_UNDERLINE = re.compile(r"^ {0,3}(?:=+|-+)[ \t]*$")
_LIST_ITEM = re.compile(r"^\s*(?:[-*+]|\d+[.)])(?:\s|$)")
_TABLE_DELIMITER = re.compile(r"^\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*$")
_CODE_SPAN = re.compile(r"(?<!`)(`+)(?!`)(.+?)(?<!`)\1(?!`)", re.DOTALL)
_HTML_COMMENT = re.compile(r"<!--.*?-->", re.DOTALL)
_HTML_ID = re.compile(r"<[A-Za-z][^>]*?\b(?:id|name)\s*=\s*[\"']([^\"']+)[\"']", re.IGNORECASE)
_HTML_TARGET = re.compile(r"<(?:a|img)\b[^>]*?\b(?:href|src)\s*=\s*[\"']([^\"']+)[\"']", re.IGNORECASE)
_REF_DEFINITION = re.compile(r"^ {0,3}\[(?!\^)[^\]]+\]:[ \t]*(<[^>\n]*>|\S+)")
_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.\-]+:")
_LINE_ANCHOR = re.compile(r"^L\d+(?:C\d+)?(?:-L\d+(?:C\d+)?)?$")
_ESCAPED = re.compile(r"\\([!-/:-@\[-`{-~])")
_UNESCAPED_PIPE = re.compile(r"(?<!\\)\|")


@dataclass
class ParsedDoc:
    anchors: set[str] = field(default_factory=set)
    links: list[tuple[int, str]] = field(default_factory=list)  # (line, raw destination)
    errors: list[str] = field(default_factory=list)  # "line: message"


@dataclass
class DocsReport:
    documents: int = 0
    links: int = 0  # relative links checked
    errors: list[str] = field(default_factory=list)  # "path:line: message"


def slugify(text: str) -> str:
    """GitHub's heading slug: lowercase, drop punctuation/symbols, spaces to hyphens."""
    out = []
    for ch in text.lower():
        if ch == " " or ch == "-":
            out.append("-")
        elif unicodedata.category(ch)[0] in "LMN" or unicodedata.category(ch) == "Pc":
            out.append(ch)
    return "".join(out)


class Slugger:
    """github-slugger duplicate handling: ``x``, ``x-1``, ``x-2`` ..."""

    def __init__(self) -> None:
        self._seen: dict[str, int] = {}

    def slug(self, text: str) -> str:
        original = slugify(text)
        result = original
        while result in self._seen:
            self._seen[original] += 1
            result = f"{original}-{self._seen[original]}"
        self._seen[result] = 0
        return result


def _strip_inline_markup(text: str) -> str:
    previous = None
    while previous != text:  # nested images inside links
        previous = text
        text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\[[^\]]*\]", r"\1", text)
    text = re.sub(r"<[A-Za-z/!][^>]*>", "", text)
    text = re.sub(r"(?<!\w)_+(?=\S)(.+?)(?<=\S)_+(?!\w)", r"\1", text)
    return html.unescape(_ESCAPED.sub(r"\1", text))


def heading_text(raw: str) -> str:
    """Approximate the rendered text of an ATX/setext heading's inline Markdown."""
    parts: list[str] = []
    pos = 0
    for match in _CODE_SPAN.finditer(raw):
        parts.append(_strip_inline_markup(raw[pos : match.start()]))
        code = match.group(2)
        if len(code) >= 2 and code[0] == code[-1] == " " and code.strip():
            code = code[1:-1]
        parts.append(code)
        pos = match.end()
    parts.append(_strip_inline_markup(raw[pos:]))
    return "".join(parts)


def _blank_keep_newlines(text: str) -> str:
    return re.sub(r"[^\n]", " ", text)


def _mask_code_spans(lines: list[str]) -> list[str]:
    """Blank inline code spans (they cannot cross a blank line) keeping line structure."""
    out: list[str] = []
    group: list[str] = []

    def flush() -> None:
        if group:
            masked = _CODE_SPAN.sub(lambda m: _blank_keep_newlines(m.group(0)), "\n".join(group))
            out.extend(masked.split("\n"))
            group.clear()

    for line in lines:
        if line.strip():
            group.append(line)
        else:
            flush()
            out.append(line)
    flush()
    return out


def _parse_destination(text: str, pos: int) -> tuple[str | None, int]:
    """Parse ``dest "title")`` starting just after ``](``; returns (dest, end) or (None, pos)."""
    n = len(text)
    k = pos
    while k < n and text[k] in " \t\n":
        k += 1
    start = k
    if k < n and text[k] == "<":
        end = text.find(">", k + 1)
        if end == -1 or "\n" in text[k:end]:
            return None, pos
        dest = text[k + 1 : end]
        k = end + 1
    else:
        depth = 0
        while k < n:
            ch = text[k]
            if ch == "\\" and k + 1 < n:
                k += 2
                continue
            if ch in " \t\n":
                break
            if ch == "(":
                depth += 1
            elif ch == ")":
                if depth == 0:
                    break
                depth -= 1
            k += 1
        dest = text[start:k]
    while k < n and text[k] in " \t\n":
        k += 1
    if k < n and text[k] in "\"'(":
        closer = ")" if text[k] == "(" else text[k]
        end = text.find(closer, k + 1)
        if end == -1:
            return None, pos
        k = end + 1
        while k < n and text[k] in " \t\n":
            k += 1
    if k >= n or text[k] != ")":
        return None, pos
    return dest, k + 1


def _inline_destinations(text: str) -> Iterable[tuple[int, str]]:
    i = 0
    while True:
        j = text.find("](", i)
        if j == -1:
            return
        i = j + 2
        if j > 0 and text[j - 1] == "\\":
            continue
        dest, end = _parse_destination(text, j + 2)
        if dest is None:
            continue
        yield j, dest
        i = end


def _cell_count(row: str) -> int:
    row = row.strip()
    if row.startswith("|"):
        row = row[1:]
    if row.endswith("|") and not row.endswith("\\|"):
        row = row[:-1]
    return len(_UNESCAPED_PIPE.split(row))


def _strip_quote(line: str) -> str:
    return _QUOTE_PREFIX.sub("", line)


def parse_markdown(text: str) -> ParsedDoc:
    doc = ParsedDoc()
    raw_lines = text.lstrip("\ufeff").splitlines()

    # 1. Blank fenced code blocks; an unterminated fence is an error.
    lines: list[str] = []
    fence: tuple[str, int, int] | None = None  # (char, length, opening line)
    for number, line in enumerate(raw_lines, 1):
        match = _FENCE.match(line)
        if fence is None:
            if match and not (match.group(1)[0] == "`" and "`" in match.group(2)):
                fence = (match.group(1)[0], len(match.group(1)), number)
                lines.append("")
            else:
                lines.append(line)
        else:
            lines.append("")
            if match and match.group(1)[0] == fence[0] and len(match.group(1)) >= fence[1] and not match.group(2).strip():
                fence = None
    if fence is not None:
        doc.errors.append(f"{fence[2]}: unclosed code fence ({fence[0] * fence[1]})")

    # 2. Blank HTML comments (line structure preserved).
    lines = _HTML_COMMENT.sub(lambda m: _blank_keep_newlines(m.group(0)), "\n".join(lines)).split("\n")

    # 3. Headings -> anchors, tables -> column consistency.
    slugger = Slugger()
    quoted = [_strip_quote(line) for line in lines]
    for index, line in enumerate(quoted):
        match = _ATX_HEADING.match(line)
        if match:
            content = _ATX_CLOSER.sub("", match.group(2) or "").strip()
            if content:
                doc.anchors.add(slugger.slug(heading_text(content)))
            continue
        previous = quoted[index - 1] if index else ""
        if (
            _SETEXT_UNDERLINE.match(line)
            and previous.strip()
            and not _ATX_HEADING.match(previous)
            and not _LIST_ITEM.match(previous)
            and "|" not in previous
            and not _SETEXT_UNDERLINE.match(previous)
        ):
            doc.anchors.add(slugger.slug(heading_text(previous.strip())))

    index = 1
    while index < len(quoted):
        line = quoted[index]
        if "|" in line and _TABLE_DELIMITER.match(line) and quoted[index - 1].strip():
            expected = _cell_count(line)
            header = _cell_count(quoted[index - 1])
            if header != expected:
                doc.errors.append(
                    f"{index + 1}: table delimiter row has {expected} cell(s) but its header row has {header}"
                )
            row = index + 1
            while row < len(quoted) and quoted[row].strip() and not _ATX_HEADING.match(quoted[row]):
                count = _cell_count(quoted[row])
                if count != expected:
                    doc.errors.append(f"{row + 1}: table row has {count} cell(s), expected {expected}")
                row += 1
            index = row
            continue
        index += 1

    # 4. Links, outside code. Code spans are blanked so `[x](y)` examples are ignored.
    masked_lines = _mask_code_spans(lines)
    masked = "\n".join(masked_lines)
    for match in _HTML_ID.finditer(masked):
        doc.anchors.add(match.group(1))
    for offset, dest in _inline_destinations(masked):
        doc.links.append((masked.count("\n", 0, offset) + 1, dest))
    for match in _HTML_TARGET.finditer(masked):
        doc.links.append((masked.count("\n", 0, match.start()) + 1, match.group(1)))
    for number, line in enumerate(lines, 1):
        match = _REF_DEFINITION.match(line)
        if match:
            dest = match.group(1)
            doc.links.append((number, dest[1:-1] if dest.startswith("<") else dest))
    doc.links.sort(key=lambda item: item[0])
    return doc


def _check_link(
    source: str,
    dest: str,
    tracked: Collection[str],
    directories: Collection[str],
    docs: Mapping[str, ParsedDoc],
) -> str | None:
    """Return an error message for a broken relative link, ``None`` if fine or external."""
    dest = html.unescape(_ESCAPED.sub(r"\1", dest.strip()))
    if not dest or _SCHEME.match(dest) or dest.startswith("//"):
        return None
    path, _, fragment = dest.partition("#")
    path = unquote(path.partition("?")[0])
    fragment = unquote(fragment)
    if path:
        if path.startswith("/"):
            resolved = posixpath.normpath(path.lstrip("/"))
        else:
            resolved = posixpath.normpath(posixpath.join(posixpath.dirname(source), path))
        if resolved == ".." or resolved.startswith("../"):
            return f"link '{dest}' escapes the repository"
        if resolved == ".":
            return None  # the repository root directory
        if resolved not in tracked and resolved not in directories:
            return f"link target '{resolved}' is not a tracked file or directory (from '{dest}')"
    else:
        resolved = source
    if not fragment or fragment == "top" or resolved in directories and resolved not in tracked:
        return None
    if resolved.lower().endswith(MARKDOWN_SUFFIXES):
        target = docs.get(resolved)
        if target is None:
            return None
        if fragment in target.anchors or fragment.removeprefix("user-content-") in target.anchors:
            return None
        return f"anchor '#{fragment}' not found in '{resolved}' (from '{dest}')"
    if _LINE_ANCHOR.match(fragment):
        return None
    return f"fragment '#{fragment}' is not a line anchor of non-Markdown file '{resolved}' (from '{dest}')"


def check_markdown(documents: Mapping[str, str], tracked: Iterable[str]) -> DocsReport:
    """Validate ``documents`` (repo-relative POSIX path -> Markdown text) against ``tracked`` paths."""
    tracked_set = set(tracked)
    directories = {p for path in tracked_set for p in _parents(path)}
    parsed = {path: parse_markdown(text) for path, text in documents.items()}
    report = DocsReport(documents=len(documents))
    for path in sorted(parsed):
        doc = parsed[path]
        report.errors.extend(f"{path}:{message}" for message in doc.errors)
        for line, dest in doc.links:
            clean = html.unescape(dest.strip())
            if clean and not _SCHEME.match(clean) and not clean.startswith("//"):
                report.links += 1
            problem = _check_link(path, dest, tracked_set, directories, parsed)
            if problem:
                report.errors.append(f"{path}:{line}: {problem}")
    return report


def _parents(path: str) -> Iterable[str]:
    parent = posixpath.dirname(path)
    while parent:
        yield parent
        parent = posixpath.dirname(parent)


def run_docs(args: argparse.Namespace) -> int:
    root = args.root.resolve()
    tracked = [p for p in git(root, "ls-files", "-z").split("\0") if p]
    documents: dict[str, str] = {}
    errors: list[str] = []
    for path in tracked:
        if not path.lower().endswith(MARKDOWN_SUFFIXES):
            continue
        file = root / path
        if not file.is_file():
            continue
        try:
            documents[path] = file.read_bytes().decode("utf-8")
        except UnicodeDecodeError as exc:
            errors.append(f"{path}:1: not valid UTF-8 ({exc.reason})")
    report = check_markdown(documents, tracked)
    errors.extend(report.errors)
    if errors:
        for message in errors:
            print(message, file=sys.stderr)
        print(f"docs: FAIL - {len(errors)} problem(s) in {report.documents} Markdown file(s)", file=sys.stderr)
        return 1
    print(f"docs: OK - {report.documents} Markdown file(s), {report.links} relative link(s) checked")
    return 0


# -------------------------------------------------------------------------- gate


@dataclass(frozen=True)
class GateCheck:
    name: str
    ok: bool
    detail: str


def _parse_flag(value: str | None) -> bool | None:
    return {"true": True, "false": False}.get(value) if value is not None else None


def evaluate_gate(
    needs_json: str | None,
    rust: str | None,
    licenses: str | None,
    package: str | None,
) -> list[GateCheck]:
    """Judge the job results; every check with ``ok == False`` fails the gate."""
    checks: list[GateCheck] = []
    if needs_json is None or not needs_json.strip():
        return [GateCheck("needs", False, "CI_NEEDS_JSON is missing or empty")]
    try:
        needs = json.loads(needs_json)
    except json.JSONDecodeError as exc:
        return [GateCheck("needs", False, f"CI_NEEDS_JSON is not valid JSON: {exc}")]
    if not isinstance(needs, dict):
        return [GateCheck("needs", False, "CI_NEEDS_JSON is not a JSON object of job results")]

    def result(job: str) -> str | None:
        entry = needs.get(job)
        value = entry.get("result") if isinstance(entry, dict) else None
        return value if isinstance(value, str) else None

    def expect(job: str, expected: str, why: str) -> None:
        actual = result(job)
        shown = actual if actual is not None else "missing"
        checks.append(GateCheck(job, actual == expected, f"{why}expected {expected}, got {shown}"))

    expect("plan", "success", "")
    expect("quality", "success", "")
    if result("plan") != "success":
        checks.append(GateCheck("selection", False, "plan did not succeed, so the selection flags cannot be trusted"))
        return checks

    flags = {"CI_RUST": rust, "CI_LICENSES": licenses, "CI_PACKAGE": package}
    selected: dict[str, bool] = {}
    for env_name, raw in flags.items():
        value = _parse_flag(raw)
        if value is None:
            checks.append(GateCheck(env_name, False, f"expected 'true' or 'false', got {raw!r}"))
        else:
            selected[env_name] = value
    for job, env_name in GATE_JOBS:
        if env_name is None or env_name not in selected:
            continue
        if selected[env_name]:
            expect(job, "success", "selected; ")
        else:
            expect(job, "skipped", "not selected; ")
    known = {job for job, _ in GATE_JOBS}
    for job in sorted(set(needs) - known):
        actual = result(job)
        checks.append(GateCheck(job, actual in ("success", "skipped"), f"unexpected extra job, result {actual}"))
    return checks


def run_gate(args: argparse.Namespace, environ: Mapping[str, str]) -> int:
    checks = evaluate_gate(
        environ.get("CI_NEEDS_JSON"),
        environ.get("CI_RUST"),
        environ.get("CI_LICENSES"),
        environ.get("CI_PACKAGE"),
    )
    for check in checks:
        print(f"{'PASS' if check.ok else 'FAIL'} {check.name}: {check.detail}")
    failures = [c for c in checks if not c.ok]
    if failures:
        print(f"Required checks: FAIL ({len(failures)} problem(s))")
        return 1
    print("Required checks: PASS")
    return 0


# --------------------------------------------------------------------------- CLI


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="ci.py", description="Animatic CI helpers.")
    parser.add_argument("--root", type=Path, default=DEFAULT_ROOT, help="repository root (default: inferred from this script)")
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("plan", help="decide which expensive CI jobs a change needs")
    p.add_argument("--base", help="commit to diff against (required unless --force)")
    p.add_argument("--head", default="HEAD", help="commit to diff (default: HEAD)")
    p.add_argument("--force", action="store_true", help="select every job without consulting git")
    p.add_argument("--github-output", type=Path, help="append rust=/licenses= lines to this GITHUB_OUTPUT file")

    sub.add_parser("docs", help="validate tracked Markdown links, anchors, fences and tables")
    sub.add_parser("gate", help="judge the needs results (CI_NEEDS_JSON, CI_RUST, CI_LICENSES, CI_PACKAGE)")
    return parser


def main(argv: list[str] | None = None, environ: Mapping[str, str] | None = None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")  # type: ignore[union-attr]
        except (AttributeError, ValueError):
            pass
    args = build_parser().parse_args(argv)
    try:
        if args.command == "plan":
            return run_plan(args)
        if args.command == "docs":
            return run_docs(args)
        return run_gate(args, os.environ if environ is None else environ)
    except CiError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
