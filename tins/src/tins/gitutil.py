"""Thin wrappers over git, gh and the shell.

Two rules are enforced here rather than left to each command:
remotes are always addressed by their https URL (ssh is not reachable from
this machine), and nothing mutating ever runs in a shared checkout.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

REMOTE_RE = re.compile(
    r"(?:git@github\.com:|https://github\.com/)(?P<org>[^/]+)/(?P<name>[^/\s]+?)(?:\.git)?$"
)


class CommandError(RuntimeError):
    pass


def run(
    args: list[str],
    cwd: Path | str | None = None,
    check: bool = True,
    capture: bool = True,
    env: dict | None = None,
) -> tuple[str, str, int]:
    p = subprocess.run(
        args,
        cwd=str(cwd) if cwd else None,
        text=True,
        env=env,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )
    out = (p.stdout or "").strip()
    err = (p.stderr or "").strip()
    if check and p.returncode != 0:
        where = f" in {cwd}" if cwd else ""
        raise CommandError(f"{' '.join(args)}{where} failed [{p.returncode}]\n{err or out}")
    return out, err, p.returncode


def git(cwd: Path | str, *args: str, check: bool = True) -> str:
    return run(["git", *args], cwd=cwd, check=check)[0]


def https_url(org: str, name: str) -> str:
    return f"https://github.com/{org}/{name}"


def parse_remote(url: str) -> tuple[str, str] | None:
    m = REMOTE_RE.search(url.strip())
    return (m.group("org"), m.group("name")) if m else None


def remote_of(path: Path) -> tuple[str, str] | None:
    out, _, code = run(
        ["git", "config", "--get", "remote.origin.url"], cwd=path, check=False
    )
    return parse_remote(out) if code == 0 else None


def is_clone(path: Path) -> bool:
    """True for a real clone; False for a linked worktree (whose .git is a file)."""
    return (path / ".git").is_dir()


def is_dirty(path: Path, tracked_only: bool = True) -> bool:
    flags = ["--untracked-files=no"] if tracked_only else []
    return bool(git(path, "status", "--porcelain", *flags))


def current_branch(path: Path) -> str:
    out, _, code = run(["git", "rev-parse", "--abbrev-ref", "HEAD"], cwd=path, check=False)
    return out if code == 0 else "?"


def head(path: Path) -> str:
    """HEAD's sha, or "" for a repo with no commits yet."""
    out, _, code = run(["git", "rev-parse", "HEAD"], cwd=path, check=False)
    return out if code == 0 else ""


def fetch_main(path: Path, org: str, name: str) -> str:
    """Fetch the remote default branch over https and return its sha."""
    git(path, "fetch", "--quiet", https_url(org, name), "main")
    return git(path, "rev-parse", "FETCH_HEAD")


def add_worktree(repo: Path, target: Path, ref: str, branch: str | None = None) -> Path:
    if target.exists():
        return target
    args = ["worktree", "add", "--quiet"]
    if branch:
        args += ["-b", branch]
    else:
        args += ["--detach"]
    git(repo, *args, str(target), ref)
    return target


def remove_worktree(repo: Path, target: Path) -> None:
    run(["git", "worktree", "remove", "--force", str(target)], cwd=repo, check=False)


def update_remote_ref(path: Path, sha: str, branch: str = "main") -> None:
    """Point refs/remotes/origin/<branch> at a sha we fetched over https.

    `shelf publish` refuses to publish a HEAD it cannot see on a
    remote-tracking branch, and origin itself is an unreachable ssh URL.
    """
    git(path, "update-ref", f"refs/remotes/origin/{branch}", sha)


def commit(
    path: Path, message: str, author_name: str, author_email: str, add_untracked: bool = False
) -> str:
    git(path, "add", "-A" if add_untracked else "-u")
    run(
        [
            "git",
            "-c",
            f"user.name={author_name}",
            "-c",
            f"user.email={author_email}",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
        cwd=path,
    )
    return head(path)


def push(path: Path, org: str, name: str, branch: str, force_with_lease: str | None = None) -> None:
    args = ["push", "--quiet"]
    if force_with_lease:
        args.append(f"--force-with-lease={branch}:{force_with_lease}")
    git(path, *args, https_url(org, name), branch)


def existing_pr(org: str, name: str, branch: str) -> str | None:
    out, _, code = run(
        [
            "gh", "pr", "list",
            "--repo", f"{org}/{name}",
            "--head", branch,
            "--state", "open",
            "--json", "url",
            "--jq", ".[0].url // empty",
        ],
        check=False,
    )
    return out or None if code == 0 else None


def create_pr(org: str, name: str, branch: str, title: str, body: str) -> str:
    if url := existing_pr(org, name, branch):
        return url
    out, _, _ = run(
        [
            "gh", "pr", "create",
            "--repo", f"{org}/{name}",
            "--base", "main",
            "--head", branch,
            "--title", title,
            "--body", body,
        ]
    )
    return out.splitlines()[-1] if out else ""


PR_FIELDS = (
    "number,title,url,headRefName,headRefOid,baseRefName,isDraft,isCrossRepository,"
    "mergeable,mergeStateStatus,author,statusCheckRollup"
)


def open_prs(org: str, name: str) -> list[dict] | None:
    """Every open PR of a repo, with the fields `tins merge` judges it on.

    None means the listing failed. A repo we could not ask about must not
    read as a repo with nothing open — that is a silent gap in a report
    whose whole job is to be exhaustive.
    """
    out, _, code = run(
        ["gh", "pr", "list", "--repo", f"{org}/{name}", "--state", "open",
         "--limit", "100", "--json", PR_FIELDS],
        check=False,
    )
    if code != 0:
        return None
    try:
        return json.loads(out) if out else []
    except json.JSONDecodeError:
        return None


def pr_files(org: str, name: str, number: int) -> list[dict] | None:
    """The changed files of a PR, each with its patch.

    Returns None rather than [] when the call fails: an empty changed-file
    set and an unanswered request must not look alike to a validator whose
    whole job is to refuse what it cannot see. `--paginate` matters because
    the endpoint returns thirty files a page, and a truncated file list
    would be a diff we only half read.
    """
    out, _, code = run(
        ["gh", "api", "--paginate", "--slurp", f"repos/{org}/{name}/pulls/{number}/files"],
        check=False,
    )
    if code != 0:
        return None
    try:
        pages = json.loads(out)  # --slurp gives one array per page
    except json.JSONDecodeError:
        return None
    if not isinstance(pages, list):
        return None
    return [f for page in pages for f in page]


def file_at_ref(org: str, name: str, path: str, ref: str) -> str | None:
    """A file's text at a revision, or None if the repo has no such file."""
    out, _, code = run(
        ["gh", "api", "-H", "Accept: application/vnd.github.raw",
         f"repos/{org}/{name}/contents/{path}?ref={ref}"],
        check=False,
    )
    return out if code == 0 else None


def merge_pr(
    org: str, name: str, number: int, method: str = "squash", head_sha: str | None = None
) -> tuple[str, int]:
    """Merge one PR, refusing if its head has moved since we validated it.

    `--match-head-commit` is the whole reason this is safe to automate: the
    validation read one revision, and without the guard a push landing
    between the read and the merge would merge a diff nobody checked.
    """
    args = ["gh", "pr", "merge", str(number), "--repo", f"{org}/{name}", f"--{method}"]
    if head_sha:
        args += ["--match-head-commit", head_sha]
    out, err, code = run(args, check=False)
    return (out or err), code


def commit_message(title: str, body: str, co_authored_by: str) -> str:
    """Build a commit message.

    The only trailer is Co-Authored-By. No session URL, ever: the link
    resolves for one person and these repos are public.
    """
    parts = [title.strip()]
    if body.strip():
        parts.append(body.strip())
    if co_authored_by:
        parts.append(f"Co-Authored-By: {co_authored_by}")
    return "\n\n".join(parts) + "\n"
