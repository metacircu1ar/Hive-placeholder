//! The read-only git policy: exact allow and deny tables, and the rendering.
use super::*;

#[test]
fn reading_commands_are_accepted() {
    for command in [
        "git status",
        "git status --porcelain",
        "git --no-pager diff --stat",
        "git -P log --oneline -n 20",
        "git diff HEAD~1 -- src/lib.rs",
        "git show 'HEAD:path with spaces/file.rs'",
        "git log --format=\"%h %an %s\"",
        "git blame -L 10,20 src/main.rs",
        "git grep -n -C 3 needle",
        "git ls-files --others --exclude-standard",
        "git rev-parse --show-toplevel",
        "git cat-file -p HEAD",
        "git branch",
        "git branch --show-current",
        "git branch -a -v",
        "git branch --list 'feature/*'",
        "git stash list",
        "git stash show -p",
        "git worktree list --porcelain",
        "git for-each-ref '--format=%(refname)' refs/heads",
        "git diff --text",
        "git rev-list --filter=blob:none HEAD",
        "git log --diff-merges=first-parent",
        "git status --short",
    ] {
        assert!(parse(command).is_some(), "{command}");
    }
}

#[test]
fn shell_syntax_other_programs_and_writes_are_refused() {
    for command in [
        "",
        "git",
        "ls",
        "GIT_DIR=x git status",
        "git status; touch pwned",
        "git status && rm -rf .",
        "git status | sh",
        "git status > out",
        "git log $(id)",
        "git log `id`",
        "git log \\\n --oneline",
        "git status\ntouch pwned",
        "git status\ttouch",
        "git log \"$HOME\"",
        "git log 'unterminated",
        "git -C /etc status",
        "git -c core.pager=sh log",
        "git --git-dir=/x status",
        "git --work-tree=/x status",
        "git --exec-path=/x status",
        "git commit -m x",
        "git add .",
        "git checkout main",
        "git fetch",
        "git config core.pager",
        "git tag",
        "git range-diff a...b",
        "git branch new-branch",
        "git branch -d main",
        "git branch -m main other",
        "git stash",
        "git stash pop",
        "git worktree add ../x",
        "git log --output=log.txt",
        "git diff --outp=diff.txt",
        "git format-patch --output-directory x",
        "git diff --ext-diff",
        "git show --textconv HEAD:file",
        "git cat-file --filters HEAD:file",
        "git log --show-signature",
        "git log --remerge-diff",
        "git log --format=%G?",
        "git for-each-ref '--format=%(signature)'",
        "git for-each-ref --format=%(refname)",
        "git grep -O needle",
        "git grep -nOless needle",
        "git grep --open-files-in-pager needle",
        "git log --format=%+G?",
        "git log --format=%-GS",
        "git log '--format=% GK'",
        "git for-each-ref '--format=%(*signature)'",
        "git for-each-ref '--format=%(*signature:grade)'",
        "git log --diff-merges=remerge",
        "git log --diff-merges=r",
        "git log --diff=remerge",
        "git status -v",
        "git status -sv",
        "git status --verbose",
        "git status --verb",
    ] {
        assert_eq!(parse(command), None, "{command:?}");
    }
    assert_eq!(parse(&format!("git log {}", "x ".repeat(200))), None);
    assert_eq!(parse(&format!("git log {}", "x".repeat(MAX_BYTES))), None);
}

#[test]
fn rendering_quotes_every_word_and_adds_program_guards() {
    assert_eq!(
        parse("git status --porcelain").unwrap().render(),
        format!("{ENVIRONMENT} exec git --no-pager -c core.fsmonitor=false 'status' '--porcelain'")
    );
    assert_eq!(
        parse("git -P log -p -- \"src/a b.rs\"").unwrap().render(),
        format!(
            "{ENVIRONMENT} exec git --no-pager -c core.fsmonitor=false 'log' '--no-ext-diff' \
             '--no-textconv' '--no-show-signature' '-p' '--' 'src/a b.rs'"
        )
    );
    assert_eq!(
        parse("git show 'it'\"s\"").unwrap().render(),
        format!(
            "{ENVIRONMENT} exec git --no-pager -c core.fsmonitor=false 'show' '--no-ext-diff' '--no-textconv' '--no-show-signature' 'its'"
        )
    );
    assert_eq!(
        parse("git log --grep='don'\\''t'"),
        None,
        "a backslash is refused even inside the agent's own quoting"
    );
    assert_eq!(
        parse("git stash show -p").unwrap().render(),
        format!(
            "{ENVIRONMENT} exec git --no-pager -c core.fsmonitor=false 'stash' 'show' '--no-ext-diff' '--no-textconv' '-p'"
        )
    );
    // An apostrophe inside a word survives the rendering's own quoting.
    assert_eq!(
        parse("git log --author=\"O Brien\" 'it''s'").unwrap().words,
        ["log", "--author=O Brien", "its"]
    );
    assert_eq!(quote("it's"), r"'it'\''s'");
}
