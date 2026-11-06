"""Behavior tests for scripts/ci.py using synthetic git repositories and documents only.

Run with:  python -m unittest discover -s scripts -p "test_ci.py"
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci  # noqa: E402

# Keep git hermetic: no user/system configuration, no signing, no CRLF conversion.
os.environ.update(
    GIT_CONFIG_GLOBAL=os.devnull,
    GIT_CONFIG_NOSYSTEM="1",
    GIT_AUTHOR_NAME="Fixture",
    GIT_AUTHOR_EMAIL="fixture@example.invalid",
    GIT_COMMITTER_NAME="Fixture",
    GIT_COMMITTER_EMAIL="fixture@example.invalid",
)

try:
    subprocess.run(["git", "--version"], capture_output=True, check=True)
    HAVE_GIT = True
except (OSError, subprocess.CalledProcessError):
    HAVE_GIT = False

RUST = (True, False)
LICENSES = (False, True)
BOTH = (True, True)
NEITHER = (False, False)


def run_cli(*argv: str, environ: dict[str, str] | None = None) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = ci.main(list(argv), environ=environ)
    return code, out.getvalue(), err.getvalue()


class Fixture:
    """A throwaway git repository; every commit stages the whole working tree."""

    def __init__(self, test: unittest.TestCase) -> None:
        tmp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        test.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.git("init", "-q")

    def git(self, *args: str) -> str:
        proc = subprocess.run(
            ["git", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false", "-C", str(self.root), *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        if proc.returncode != 0:
            raise AssertionError(f"git {args} failed: {proc.stderr}")
        return proc.stdout.strip()

    def write(self, path: str, text: str = "x\n") -> None:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8", newline="\n")

    def remove(self, path: str) -> None:
        (self.root / path).unlink()

    def commit_all(self) -> str:
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", "fixture")
        return self.git("rev-parse", "HEAD")

    def apply(self, changes: dict[str, str | None]) -> str:
        for path, text in changes.items():
            if text is None:
                self.remove(path)
            else:
                self.write(path, text)
        return self.commit_all()

    def plan(self, base: str, head: str = "HEAD", *extra: str) -> tuple[int, str, str]:
        return run_cli("--root", str(self.root), "plan", "--base", base, "--head", head, *extra)


def planned(code_out_err: tuple[int, str, str]) -> tuple[bool, bool]:
    code, out, err = code_out_err
    assert code == 0, err
    data = json.loads(out)  # stdout must be the JSON document and nothing else
    assert set(data) == {"rust", "licenses"}
    return data["rust"], data["licenses"]


class ClassifyTests(unittest.TestCase):
    def test_selection_by_path(self):
        cases = {
            # documentation only
            "README.md": NEITHER,
            "CHANGELOG.md": NEITHER,
            "AGENTS.md": NEITHER,
            "docs/architecture.md": NEITHER,
            "wiki/Home.md": NEITHER,
            "wiki/_Sidebar.md": NEITHER,
            "docs/img/screenshot.png": NEITHER,
            "notes/idea.md": NEITHER,
            # unknown docs payload fails closed
            "docs/tool.exe": BOTH,
            # manifests and lockfile
            "Cargo.toml": BOTH,
            "Cargo.lock": BOTH,
            "crates/animatic-core/Cargo.toml": BOTH,
            # Rust sources and everything else under crates/ (include_str! consumers)
            "crates/animatic-core/src/lib.rs": RUST,
            "crates/animatic-core/src/db/schema.sql": RUST,
            "crates/animatic-core/src/rename/default_pattern.txt": RUST,
            "crates/animatic/README.md": RUST,
            "rustfmt.toml": RUST,
            # licence policy inputs
            "about.toml": LICENSES,
            "about.hbs": LICENSES,
            "LICENSE": LICENSES,
            # packaging, CI and toolchain are critical
            "scripts/release.py": BOTH,
            "scripts/test_release.py": BOTH,
            ".github/workflows/ci.yml": BOTH,
            ".github/actions/setup/action.yml": BOTH,
            ".github/dependabot.yml": BOTH,
            ".cargo/config.toml": BOTH,
            "rust-toolchain.toml": BOTH,
            # other Python tooling is quality-only
            "scripts/ci.py": NEITHER,
            "scripts/test_ci.py": NEITHER,
            "scripts/another_tool.py": NEITHER,
            # unknown things fail closed
            "scripts/helper.sh": BOTH,
            ".gitignore": BOTH,
            "mystery.bin": BOTH,
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(ci.classify_path(path), expected)

    def test_mixed_changes_or_together(self):
        self.assertEqual(ci.select_paths(["README.md", "scripts/ci.py"]), NEITHER)
        self.assertEqual(ci.select_paths(["README.md", "crates/a/src/lib.rs"]), RUST)
        self.assertEqual(ci.select_paths(["crates/a/src/lib.rs", "about.toml"]), BOTH)
        self.assertEqual(ci.select_paths(["README.md", "about.hbs"]), LICENSES)
        self.assertEqual(ci.select_paths(["README.md", "mystery.bin"]), BOTH)

    def test_no_changed_paths_selects_nothing(self):
        self.assertEqual(ci.select_paths([]), NEITHER)


@unittest.skipUnless(HAVE_GIT, "git is not available")
class PlanTests(unittest.TestCase):
    def setUp(self):
        self.repo = Fixture(self)
        self.repo.write("README.md")
        self.repo.write("docs/guide.md")
        self.repo.write("Cargo.toml")
        self.repo.write("Cargo.lock")
        self.repo.write("crates/core/Cargo.toml")
        self.repo.write("crates/core/src/lib.rs")
        self.repo.write("scripts/release.py")
        self.repo.write("scripts/ci.py")
        self.base = self.repo.commit_all()

    def test_docs_only_change_skips_expensive_jobs(self):
        self.repo.apply({"docs/guide.md": "changed\n", "README.md": "changed\n"})
        self.assertEqual(planned(self.repo.plan(self.base)), NEITHER)

    def test_quality_script_change_skips_expensive_jobs(self):
        self.repo.apply({"scripts/ci.py": "changed\n"})
        self.assertEqual(planned(self.repo.plan(self.base)), NEITHER)

    def test_rust_source_change_selects_rust_only(self):
        self.repo.apply({"crates/core/src/lib.rs": "changed\n"})
        self.assertEqual(planned(self.repo.plan(self.base)), RUST)

    def test_manifest_and_lockfile_changes_select_both(self):
        for path in ("Cargo.lock", "Cargo.toml", "crates/core/Cargo.toml"):
            with self.subTest(path=path):
                before = self.repo.git("rev-parse", "HEAD")
                self.repo.apply({path: f"changed {path}\n"})
                self.assertEqual(planned(self.repo.plan(before)), BOTH)

    def test_packaging_script_and_workflow_changes_select_both(self):
        self.repo.apply({"scripts/release.py": "changed\n"})
        self.assertEqual(planned(self.repo.plan(self.base)), BOTH)
        before = self.repo.git("rev-parse", "HEAD")
        self.repo.apply({".github/workflows/ci.yml": "on: push\n"})
        self.assertEqual(planned(self.repo.plan(before)), BOTH)

    def test_mixed_docs_and_rust_changes_aggregate(self):
        self.repo.apply({"docs/guide.md": "changed\n", "crates/core/src/lib.rs": "changed\n"})
        self.assertEqual(planned(self.repo.plan(self.base)), RUST)

    def test_deleting_a_rust_file_is_still_a_rust_change(self):
        self.repo.apply({"crates/core/src/lib.rs": None})
        self.assertEqual(planned(self.repo.plan(self.base)), RUST)

    def test_deleting_a_manifest_selects_both(self):
        self.repo.apply({"crates/core/Cargo.toml": None})
        self.assertEqual(planned(self.repo.plan(self.base)), BOTH)

    def test_renaming_rust_source_out_of_crates_still_selects_rust(self):
        self.repo.git("mv", "crates/core/src/lib.rs", "docs/lib.md")
        self.repo.commit_all()
        self.assertEqual(planned(self.repo.plan(self.base)), RUST)

    def test_renaming_docs_into_crates_selects_rust(self):
        self.repo.git("mv", "docs/guide.md", "crates/core/guide.md")
        self.repo.commit_all()
        self.assertEqual(planned(self.repo.plan(self.base)), RUST)

    def test_pure_docs_rename_selects_nothing(self):
        self.repo.git("mv", "docs/guide.md", "docs/manual.md")
        self.repo.commit_all()
        self.assertEqual(planned(self.repo.plan(self.base)), NEITHER)

    def test_unchanged_head_selects_nothing(self):
        self.assertEqual(planned(self.repo.plan(self.base)), NEITHER)

    def test_range_covers_all_commits_between_base_and_head(self):
        self.repo.apply({"docs/guide.md": "one\n"})
        self.repo.apply({"crates/core/src/lib.rs": "two\n"})
        self.repo.apply({"docs/guide.md": "three\n"})
        self.assertEqual(planned(self.repo.plan(self.base)), RUST)

    def test_head_defaults_to_head_commit(self):
        self.repo.apply({"crates/core/src/lib.rs": "changed\n"})
        result = run_cli("--root", str(self.repo.root), "plan", "--base", self.base)
        self.assertEqual(planned(result), RUST)

    def test_explicit_head_limits_the_range(self):
        first = self.repo.apply({"docs/guide.md": "one\n"})
        self.repo.apply({"crates/core/src/lib.rs": "two\n"})
        self.assertEqual(planned(self.repo.plan(self.base, first)), NEITHER)

    def test_force_selects_everything_without_a_repository(self):
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            result = run_cli("--root", tmp, "plan", "--force")
        self.assertEqual(planned(result), BOTH)

    def test_force_ignores_unusable_revisions(self):
        result = run_cli("--root", str(self.repo.root), "plan", "--force", "--base", "f" * 40, "--head", "nope")
        self.assertEqual(planned(result), BOTH)

    def test_missing_base_without_force_is_an_error(self):
        code, out, err = run_cli("--root", str(self.repo.root), "plan")
        self.assertEqual(code, 1)
        self.assertEqual(out, "")
        self.assertIn("--base is required", err)

    def test_empty_base_without_force_is_an_error(self):
        code, out, err = run_cli("--root", str(self.repo.root), "plan", "--base", "")
        self.assertEqual((code, out), (1, ""))
        self.assertIn("--base is required", err)

    def test_all_zero_base_runs_everything(self):
        self.assertEqual(planned(self.repo.plan("0" * 40)), BOTH)

    def test_unavailable_base_fails_instead_of_skipping(self):
        code, out, err = self.repo.plan("1" * 40)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("unavailable", err)

    def test_unavailable_head_fails(self):
        code, out, err = self.repo.plan(self.base, "2" * 40)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("head commit", err)

    def test_option_like_revision_is_rejected(self):
        code, out, err = run_cli("--root", str(self.repo.root), "plan", "--base=-x")
        self.assertEqual((code, out), (1, ""))
        self.assertIn("not a valid revision", err)

    def test_not_a_repository_fails_without_force(self):
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            code, out, err = run_cli("--root", tmp, "plan", "--base", self.base)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("error:", err)

    def test_github_output_is_appended(self):
        self.repo.apply({"crates/core/src/lib.rs": "changed\n"})
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            target = Path(tmp) / "github_output"
            target.write_text("existing=1\n", encoding="utf-8", newline="\n")
            planned(self.repo.plan(self.base, "HEAD", "--github-output", str(target)))
            planned(run_cli("--root", tmp, "plan", "--force", "--github-output", str(target)))
            content = target.read_bytes().decode("utf-8")
        self.assertEqual(content, "existing=1\nrust=true\nlicenses=false\nrust=true\nlicenses=true\n")

    def test_github_output_is_not_written_on_failure(self):
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            target = Path(tmp) / "github_output"
            code, _, _ = self.repo.plan("1" * 40, "HEAD", "--github-output", str(target))
            self.assertEqual(code, 1)
            self.assertFalse(target.exists())

    def test_parser_rejects_unknown_arguments(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as raised:
            ci.main(["plan", "--bogus"])
        self.assertEqual(raised.exception.code, 2)


def check(documents: dict[str, str], tracked: list[str] | None = None) -> list[str]:
    paths = set(documents) | set(tracked or ())
    return ci.check_markdown(documents, paths).errors


class AnchorTests(unittest.TestCase):
    def anchors(self, text: str) -> set[str]:
        return ci.parse_markdown(text).anchors

    def test_github_slugs(self):
        text = (
            "# Title\n"
            "## Install & run\n"
            "### `Code` and [a link](x.md) with *emphasis* and __strong__\n"
            "## Ünïcode Ñame\n"
            "## snake_case_name and under_score\n"
            "## Trailing hashes ##\n"
            "## Punctuation: (is) gone, 100%!\n"
        )
        self.assertEqual(
            self.anchors(text),
            {
                "title",
                "install--run",
                "code-and-a-link-with-emphasis-and-strong",
                "ünïcode-ñame",
                "snake_case_name-and-under_score",
                "trailing-hashes",
                "punctuation-is-gone-100",
            },
        )

    def test_duplicate_headings_get_numeric_suffixes(self):
        self.assertEqual(self.anchors("# Intro\n## Intro\n## Intro\n"), {"intro", "intro-1", "intro-2"})

    def test_duplicate_suffix_collision_with_a_real_heading(self):
        self.assertEqual(self.anchors("# A\n# A-1\n# A\n"), {"a", "a-1", "a-2"})

    def test_setext_headings_and_html_ids(self):
        text = "Setext Title\n============\n\nSub Title\n---------\n\n<a id=\"custom\"></a> <a name='legacy'></a>\n"
        self.assertEqual(self.anchors(text), {"setext-title", "sub-title", "custom", "legacy"})

    def test_headings_in_fences_and_comments_do_not_count(self):
        text = "```md\n# Fenced\n```\n<!--\n# Commented\n-->\n# Real\n"
        self.assertEqual(self.anchors(text), {"real"})

    def test_hash_without_space_is_not_a_heading(self):
        self.assertEqual(self.anchors("#hashtag\n"), set())

    def test_headings_inside_blockquotes_count(self):
        self.assertEqual(self.anchors("> ## Quoted\n"), {"quoted"})


class LinkTests(unittest.TestCase):
    TRACKED = ["README.md", "Cargo.toml", "docs/a.md", "docs/b.md", "crates/x/src/lib.rs", "docs/with space.md"]

    def run_docs(self, a: str, b: str = "# Section\n\n## Café au lait\n\n## A B\n") -> list[str]:
        return check({"docs/a.md": a, "docs/b.md": b, "README.md": "# Readme\n"}, self.TRACKED)

    def test_valid_links_pass(self):
        links = (
            "[b](b.md) [b section](b.md#section) [cafe](b.md#café-au-lait) [enc](b.md#caf%C3%A9-au-lait)\n"
            "[ab](b.md#a-b) [root](../README.md#readme) [self](#top) [here](a.md)\n"
            "[dir](../crates/x/src) [dir slash](../crates/x/src/) [repo](..) [abs](/Cargo.toml)\n"
            "[lines](../crates/x/src/lib.rs#L10-L20) [line](../crates/x/src/lib.rs?plain=1#L7)\n"
            "[space](<with space.md>) [space2](with%20space.md)\n"
            "[title](b.md \"a title\") [paren](b.md#section 'x')\n"
            "[ext](https://example.invalid/nope.md#nope) [mail](mailto:a@example.invalid) [proto](//example.invalid/x)\n"
            "[![badge](https://example.invalid/b.svg)](https://example.invalid)\n"
            "<https://example.invalid/auto>\n"
        )
        self.assertEqual(self.run_docs("# Top\n\n" + links), [])

    def test_self_anchor_is_checked(self):
        errors = self.run_docs("# Top\n\n[ok](#top-1) [bad](#nowhere)\n\n## Top\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("docs/a.md:3:", errors[0])
        self.assertIn("#nowhere", errors[0])

    def test_missing_file_is_reported_with_line_number(self):
        errors = self.run_docs("# T\n\ntext\n[gone](missing.md)\n")
        self.assertEqual(len(errors), 1)
        self.assertTrue(errors[0].startswith("docs/a.md:4:"), errors[0])
        self.assertIn("docs/missing.md", errors[0])

    def test_missing_image_is_reported(self):
        errors = self.run_docs("![shot](img/shot.png)\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("docs/img/shot.png", errors[0])

    def test_missing_anchor_in_other_file_is_reported(self):
        errors = self.run_docs("[x](b.md#absent)\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("#absent", errors[0])
        self.assertIn("docs/b.md", errors[0])

    def test_duplicate_heading_anchor_links(self):
        b = "# Dup\n\n## Dup\n"
        self.assertEqual(self.run_docs("[1](b.md#dup) [2](b.md#dup-1)\n", b), [])
        self.assertEqual(len(self.run_docs("[3](b.md#dup-2)\n", b)), 1)

    def test_anchor_match_is_case_sensitive(self):
        self.assertEqual(len(self.run_docs("[x](b.md#Section)\n")), 1)

    def test_link_escaping_the_repository_is_reported(self):
        errors = self.run_docs("[x](../../outside.md)\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("escapes the repository", errors[0])

    def test_non_line_fragment_on_source_file_is_reported(self):
        errors = self.run_docs("[x](../crates/x/src/lib.rs#section)\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("line anchor", errors[0])

    def test_reference_definitions_and_html_targets_are_checked(self):
        text = "[ref]: missing-ref.md\n[ok]: b.md#section\n\n<img src=\"missing-img.png\">\n<a href='b.md'>fine</a>\n"
        errors = self.run_docs(text)
        self.assertEqual(len(errors), 2)
        self.assertTrue(any("missing-ref.md" in e for e in errors))
        self.assertTrue(any("missing-img.png" in e for e in errors))

    def test_fenced_code_is_ignored_for_links_and_tables(self):
        text = (
            "```md\n[bad](missing.md)\n| a | b |\n|---|---|\n| only |\n```\n\n"
            "~~~\n[bad](missing.md)\n```\n[bad](missing.md)\n~~~\n\n"
            "````text\n```\n[bad](missing.md)\n````\n"
        )
        self.assertEqual(self.run_docs(text), [])

    def test_inline_code_and_comments_are_ignored(self):
        text = (
            "Use `[bad](missing.md)` or ``[bad](missing.md) with ` tick``.\n"
            "<!-- [bad](missing.md) -->\n"
            "<!--\n[bad](missing.md)\n-->\n"
        )
        self.assertEqual(self.run_docs(text), [])

    def test_links_after_a_code_span_on_the_same_line_are_still_checked(self):
        errors = self.run_docs("`code` then [bad](missing.md)\n")
        self.assertEqual(len(errors), 1)

    def test_prose_that_only_looks_like_a_link_is_ignored(self):
        self.assertEqual(self.run_docs("f](x y z) and a](unterminated\n"), [])

    def test_nested_image_link_checks_both_targets(self):
        errors = self.run_docs("[![a](missing.png)](missing.md)\n")
        self.assertEqual(len(errors), 2)

    def test_directory_link_needs_tracked_content(self):
        self.assertEqual(len(self.run_docs("[x](../empty-dir/)\n")), 1)

    def test_unclosed_fence_is_reported(self):
        errors = self.run_docs("# T\n\n```sh\necho hi\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("docs/a.md:3:", errors[0])
        self.assertIn("unclosed code fence", errors[0])

    def test_shorter_or_different_closing_fence_does_not_close(self):
        self.assertEqual(len(self.run_docs("````\ncode\n```\n")), 1)
        self.assertEqual(len(self.run_docs("```\ncode\n~~~\n")), 1)

    def test_info_string_closing_fence_does_not_close(self):
        self.assertEqual(len(self.run_docs("```\ncode\n```sh\n")), 1)

    def test_balanced_fences_pass(self):
        self.assertEqual(self.run_docs("```sh\necho\n```\n\n````md\n```\nx\n```\n````\n"), [])

    def test_tables(self):
        good = "| A | B |\n|---|:-:|\n| 1 | 2 |\n| 3 | 4 |\n\ntext\n"
        self.assertEqual(self.run_docs(good), [])
        self.assertEqual(self.run_docs("A | B\n--|--\n1 | 2\n"), [])
        self.assertEqual(self.run_docs("| a \\| b | c |\n|---|---|\n| 1 | 2 |\n"), [])

    def test_table_row_with_wrong_cell_count_is_reported(self):
        errors = self.run_docs("# T\n\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 |\n| 4 | 5 | 6 |\n")
        self.assertEqual(len(errors), 2)
        self.assertIn("docs/a.md:6:", errors[0])
        self.assertIn("docs/a.md:7:", errors[1])

    def test_table_header_and_delimiter_mismatch_is_reported(self):
        errors = self.run_docs("| A | B | C |\n|---|---|\n| 1 | 2 |\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("docs/a.md:2:", errors[0])

    def test_thematic_break_is_not_a_table(self):
        self.assertEqual(self.run_docs("text\n\n---\n\nmore\n"), [])

    def test_crlf_and_bom_documents(self):
        text = "\ufeff# Top\r\n\r\n[x](b.md#section)\r\n\r\n```\r\nfenced\r\n```\r\n"
        self.assertEqual(self.run_docs(text), [])

    def test_report_counts_relative_links_and_repository_urls(self):
        report = ci.check_markdown(
            {
                "a.md": "[x](b.md) [y](https://example.invalid) [z](#top) [u](https://github.com/RetroHazard/animatic/blob/main/b.md)\n",
                "b.md": "# B\n",
            },
            {"a.md", "b.md"},
        )
        self.assertEqual((report.documents, report.links, report.urls, report.errors), (2, 2, 1, []))


REPO_URL = "https://github.com/RetroHazard/animatic"


class WikiDocsTests(unittest.TestCase):
    TRACKED = ["README.md", "docs/dev.md", "crates/x/src/lib.rs", "wiki/Home.md", "wiki/Guide.md", "wiki/_Sidebar.md"]

    def run_docs(self, readme: str = "# Readme\n", guide: str = "# Guide\n\n## Start\n", **extra: str) -> list[str]:
        documents = {
            "README.md": readme,
            "docs/dev.md": "# Dev\n\n## Build\n",
            "wiki/Home.md": "## Welcome\n",
            "wiki/Guide.md": guide,
            "wiki/_Sidebar.md": "- [Home](Home.md)\n",
        }
        documents.update({f"wiki/{name}": text for name, text in extra.items()})
        return check(documents, self.TRACKED)

    def test_wiki_pages_link_each_other_and_the_repository_by_url(self):
        guide = (
            "## Start\n\n[home](Home.md#welcome) [self](#start) [side](_Sidebar.md)\n"
            f"[dev]({REPO_URL}/blob/main/docs/dev.md#build) [src]({REPO_URL}/tree/main/crates/x)\n"
            f"[line]({REPO_URL}/blob/main/crates/x/src/lib.rs#L3) [issues]({REPO_URL}/issues/new/choose)\n"
        )
        self.assertEqual(self.run_docs(guide=guide), [])

    def test_wiki_page_links_that_leave_the_wiki_are_reported(self):
        guide = "[readme](../README.md)\n[dev](../docs/dev.md#build)\n[root](/README.md)\n"
        errors = self.run_docs(guide=guide)
        self.assertEqual(len(errors), 3)
        self.assertTrue(all("leaves the wiki pages" in e for e in errors), errors)
        self.assertEqual([e.split(":")[1] for e in errors], ["1", "2", "3"])

    def test_repository_urls_are_checked_against_the_checkout(self):
        readme = (
            f"[ok]({REPO_URL}/blob/main/docs/dev.md#build)\n"
            f"[gone]({REPO_URL}/blob/main/docs/gone.md)\n"
            f"[anchor]({REPO_URL}/blob/main/docs/dev.md#absent)\n"
            f"[wiki]({REPO_URL}/wiki) [page]({REPO_URL}/wiki/Guide#start)\n"
            f"[no page]({REPO_URL}/wiki/Missing)\n"
            f"[no anchor]({REPO_URL}/wiki/Guide#absent)\n"
            f"[branch]({REPO_URL}/blob/other/docs/gone.md) [lookalike]({REPO_URL}/wikis/x)\n"
        )
        errors = self.run_docs(readme=readme)
        self.assertEqual([e.split(":")[1] for e in errors], ["2", "3", "5", "6"], errors)
        self.assertIn("docs/gone.md", errors[0])
        self.assertIn("#absent", errors[1])
        self.assertIn("wiki/Missing.md", errors[2])
        self.assertIn("wiki/Guide.md", errors[3])

    def test_wiki_holds_only_flat_packageable_pages(self):
        tracked = [*self.TRACKED, "wiki/sub/Page.md", "wiki/_Header.md", "wiki/a b.md", "wiki/shot.png", "wiki/x..y.md"]
        errors = check({"wiki/Home.md": "# Home\n"}, tracked)
        self.assertEqual(
            sorted(e.split(":")[0] for e in errors),
            ["wiki/_Header.md", "wiki/a b.md", "wiki/shot.png", "wiki/sub/Page.md", "wiki/x..y.md"],
        )

    def test_page_names_follow_the_updater_rule(self):
        self.assertTrue(ci.is_packaged_wiki_page("Rename-Language.md"))
        self.assertTrue(ci.is_packaged_wiki_page("a" * 97 + ".md"))
        self.assertFalse(ci.is_packaged_wiki_page("a" * 98 + ".md"))
        for name in ("_Sidebar.md", "-x.md", ".x.md", "a..md", "x.txt", "Ä.md"):
            self.assertFalse(ci.is_packaged_wiki_page(name), name)


class WikiLinkTests(unittest.TestCase):
    PAGES = {"Home.md", "Guide.md", "_Sidebar.md"}

    def test_page_links_lose_the_extension(self):
        text = (
            "[a](Guide.md) [b](Guide.md#start) [c](<Guide.md>) [d](Guide.md \"title\") [e]( Home.md )\n"
            "[side](_Sidebar.md)\n[ref]: Guide.md#ref\n"
        )
        expected = (
            "[a](Guide) [b](Guide#start) [c](<Guide>) [d](Guide \"title\") [e]( Home )\n"
            "[side](_Sidebar)\n[ref]: Guide#ref\n"
        )
        self.assertEqual(ci.wiki_links(text, self.PAGES), expected)

    def test_other_links_and_code_are_untouched(self):
        text = (
            "[other](Other.md) [up](../Guide.md) [url](https://example.invalid/Guide.md) [file](Guide.mdx)\n"
            "`[code](Guide.md)` <!-- [c](Guide.md) -->\n\n```md\n[fenced](Guide.md)\n```\r\n"
            "[after](Guide.md)\r\n"
        )
        expected = text.replace("[after](Guide.md)", "[after](Guide)")
        self.assertEqual(ci.wiki_links(text, self.PAGES), expected)


@unittest.skipUnless(HAVE_GIT, "git is not available")
class WikiExportTests(unittest.TestCase):
    def setUp(self):
        self.repo = Fixture(self)
        self.repo.write("wiki/Home.md", "[guide](Guide.md#start)\n")
        self.repo.write("wiki/Guide.md", "## Start\n\n[home](Home.md)\n")
        self.repo.write("wiki/_Sidebar.md", "- [Guide](Guide.md)\n")
        self.repo.write("docs/dev.md", "# Dev\n")
        self.repo.commit_all()
        self.repo.write("wiki/Draft.md", "untracked\n")
        tmp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)
        self.addCleanup(tmp.cleanup)
        self.out = Path(tmp.name) / "wiki.git"

    def export(self) -> tuple[int, str, str]:
        return run_cli("--root", str(self.repo.root), "wiki", "--out", str(self.out))

    def test_exports_tracked_pages_with_wiki_links(self):
        (self.out / ".git").mkdir(parents=True)
        code, out, err = self.export()
        self.assertEqual(code, 0, err)
        self.assertIn("3 page(s)", out)
        self.assertEqual(sorted(p.name for p in self.out.iterdir()), [".git", "Guide.md", "Home.md", "_Sidebar.md"])
        self.assertEqual((self.out / "Home.md").read_text(encoding="utf-8"), "[guide](Guide#start)\n")
        self.assertEqual((self.out / "_Sidebar.md").read_text(encoding="utf-8"), "- [Guide](Guide)\n")

    def test_refuses_an_output_directory_with_content(self):
        self.out.mkdir(parents=True)
        (self.out / "Stale.md").write_text("old\n", encoding="utf-8")
        code, out, err = self.export()
        self.assertEqual(code, 1)
        self.assertIn("must be a missing or empty directory", err)
        self.assertEqual([p.name for p in self.out.iterdir()], ["Stale.md"])

    def test_refuses_invalid_page_names(self):
        self.repo.write("wiki/sub/Page.md", "x\n")
        self.repo.commit_all()
        code, _, err = self.export()
        self.assertEqual(code, 1)
        self.assertIn("wiki/sub/Page.md", err)
        self.assertFalse(self.out.exists())


@unittest.skipUnless(HAVE_GIT, "git is not available")
class DocsCliTests(unittest.TestCase):
    def test_validates_tracked_markdown_only(self):
        repo = Fixture(self)
        repo.write("README.md", "# Home\n\n[guide](docs/guide.md#start) [lic](LICENSE) [dir](docs)\n")
        repo.write("docs/guide.md", "# Start\n\n[home](../README.md#home)\n")
        repo.write("LICENSE", "text\n")
        repo.commit_all()
        # An untracked Markdown file with a broken link is outside the checkout contract.
        repo.write("docs/untracked.md", "[bad](missing.md)\n")
        code, out, err = run_cli("--root", str(repo.root), "docs")
        self.assertEqual(code, 0, err)
        self.assertIn("2 Markdown file(s)", out)
        self.assertIn("4 relative link(s)", out)

    def test_untracked_link_target_is_a_failure(self):
        repo = Fixture(self)
        repo.write("README.md", "[local only](notes.txt)\n")
        repo.commit_all()
        repo.write("notes.txt")
        code, out, err = run_cli("--root", str(repo.root), "docs")
        self.assertEqual(code, 1)
        self.assertIn("README.md:1:", err)
        self.assertIn("notes.txt", err)

    def test_failure_lists_every_problem(self):
        repo = Fixture(self)
        repo.write("README.md", "[a](gone.md)\n\n```\nopen\n")
        repo.write("docs/x.md", "[b](../README.md#nope)\n")
        repo.commit_all()
        code, out, err = run_cli("--root", str(repo.root), "docs")
        self.assertEqual(code, 1)
        self.assertEqual(out, "")
        self.assertIn("README.md:1:", err)
        self.assertIn("README.md:3:", err)
        self.assertIn("docs/x.md:1:", err)
        self.assertIn("3 problem(s)", err)

    def test_non_utf8_markdown_is_reported(self):
        repo = Fixture(self)
        (repo.root / "README.md").write_bytes(b"# ok\n\xff\xfe\n")
        repo.commit_all()
        code, _, err = run_cli("--root", str(repo.root), "docs")
        self.assertEqual(code, 1)
        self.assertIn("README.md:1:", err)
        self.assertIn("UTF-8", err)

    def test_uppercase_extension_is_checked(self):
        repo = Fixture(self)
        repo.write("NOTES.MD", "[a](gone.md)\n")
        repo.commit_all()
        code, _, err = run_cli("--root", str(repo.root), "docs")
        self.assertEqual(code, 1)
        self.assertIn("NOTES.MD:1:", err)


def needs(**results: str) -> str:
    return json.dumps({job: {"result": result, "outputs": {}} for job, result in results.items()})


def gate_env(needs_json: str | None, rust="true", licenses="true", package="false") -> dict[str, str]:
    env = {"CI_RUST": rust, "CI_LICENSES": licenses, "CI_PACKAGE": package}
    if needs_json is not None:
        env["CI_NEEDS_JSON"] = needs_json
    return env


class GateTests(unittest.TestCase):
    def gate(self, needs_json: str | None, **flags: str) -> tuple[int, str]:
        code, out, err = run_cli("gate", environ=gate_env(needs_json, **flags))
        self.assertEqual(err, "")
        return code, out

    def assertPasses(self, needs_json: str, **flags: str) -> None:
        code, out = self.gate(needs_json, **flags)
        self.assertEqual(code, 0, out)
        self.assertIn("Required checks: PASS", out)
        self.assertNotIn("FAIL", out)

    def assertFails(self, needs_json: str | None, *reasons: str, **flags: str) -> None:
        code, out = self.gate(needs_json, **flags)
        self.assertEqual(code, 1, out)
        self.assertIn("Required checks: FAIL", out)
        for reason in reasons:
            self.assertIn(reason, out)

    def test_all_selected_jobs_succeeded(self):
        self.assertPasses(
            needs(plan="success", quality="success", notices="success", rust="success", package="success"),
            package="true",
        )

    def test_unselected_jobs_must_be_skipped(self):
        self.assertPasses(
            needs(plan="success", quality="success", notices="skipped", rust="skipped", package="skipped"),
            rust="false",
            licenses="false",
        )

    def test_partial_selection(self):
        self.assertPasses(
            needs(plan="success", quality="success", notices="skipped", rust="success", package="skipped"),
            licenses="false",
        )

    def test_selected_job_that_was_skipped_fails(self):
        self.assertFails(
            needs(plan="success", quality="success", notices="skipped", rust="success", package="skipped"),
            "FAIL notices",
            "expected success, got skipped",
        )

    def test_unselected_job_that_ran_fails(self):
        self.assertFails(
            needs(plan="success", quality="success", notices="success", rust="success", package="success"),
            "FAIL package",
            "expected skipped, got success",
        )

    def test_failed_and_cancelled_jobs_fail(self):
        for result in ("failure", "cancelled"):
            with self.subTest(result=result):
                self.assertFails(
                    needs(plan="success", quality="success", notices="success", rust=result, package="skipped"),
                    "FAIL rust",
                    f"got {result}",
                )

    def test_quality_must_succeed_even_when_everything_else_does(self):
        for result in ("failure", "cancelled", "skipped"):
            with self.subTest(result=result):
                self.assertFails(
                    needs(plan="success", quality=result, notices="success", rust="success", package="skipped"),
                    "FAIL quality",
                )

    def test_failed_plan_fails_regardless_of_flags(self):
        self.assertFails(
            needs(plan="failure", quality="success", notices="skipped", rust="skipped", package="skipped"),
            "FAIL plan",
            "FAIL selection",
            rust="",
            licenses="",
            package="",
        )

    def test_cancelled_plan_fails(self):
        self.assertFails(needs(plan="cancelled", quality="cancelled"), "FAIL plan", "FAIL quality")

    def test_missing_plan_fails(self):
        self.assertFails(
            needs(quality="success", notices="success", rust="success", package="skipped"),
            "FAIL plan",
            "got missing",
        )

    def test_missing_selected_job_fails(self):
        self.assertFails(needs(plan="success", quality="success", notices="success", package="skipped"), "FAIL rust", "got missing")

    def test_missing_unselected_job_fails(self):
        self.assertFails(
            needs(plan="success", quality="success", notices="success", rust="success"), "FAIL package", "got missing"
        )

    def test_malformed_flags_fail(self):
        base = needs(plan="success", quality="success", notices="success", rust="success", package="skipped")
        for bad in ("", "True", "1", "yes", "TRUE"):
            with self.subTest(bad=bad):
                self.assertFails(base, "FAIL CI_RUST", rust=bad)
        self.assertFails(base, "FAIL CI_PACKAGE", package="maybe")

    def test_missing_flag_environment_fails(self):
        code, out, _ = run_cli(
            "gate",
            environ={
                "CI_NEEDS_JSON": needs(plan="success", quality="success", notices="success", rust="success", package="skipped")
            },
        )
        self.assertEqual(code, 1)
        self.assertIn("FAIL CI_RUST", out)

    def test_malformed_needs_fail(self):
        self.assertFails(None, "CI_NEEDS_JSON is missing")
        self.assertFails("", "CI_NEEDS_JSON is missing")
        self.assertFails("{not json", "not valid JSON")
        self.assertFails("[]", "not a JSON object")
        self.assertFails('"success"', "not a JSON object")

    def test_malformed_job_entries_fail(self):
        self.assertFails(
            json.dumps({"plan": "success", "quality": {"result": None}}),
            "FAIL plan",
            "FAIL quality",
        )

    def test_unexpected_extra_job_must_not_have_failed(self):
        base = dict(plan="success", quality="success", notices="success", rust="success", package="skipped")
        self.assertPasses(needs(**base, extra="success"))
        self.assertFails(needs(**base, extra="failure"), "FAIL extra")

    def test_every_problem_is_reported(self):
        self.assertFails(
            needs(plan="success", quality="failure", notices="cancelled", rust="skipped", package="success"),
            "FAIL quality",
            "FAIL notices",
            "FAIL rust",
            "FAIL package",
            "4 problem(s)",
            package="false",
        )

    def test_gate_does_not_need_a_repository_or_arguments(self):
        with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
            code, out, err = run_cli(
                "--root",
                tmp,
                "gate",
                environ=gate_env(needs(plan="success", quality="success", notices="success", rust="success", package="skipped")),
            )
        self.assertEqual((code, err), (0, ""))
        self.assertIn("Required checks: PASS", out)


if __name__ == "__main__":
    unittest.main()
