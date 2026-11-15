# Audit a repository before making it public

Run this before changing a repository's visibility, and again before any
other repository of the project goes public. Plan and findings of the
first run: [`specs/018`](../../specs/018-open-source.md).

The order matters: the cheap greps come first, because one of them found a
script that would have given a fixed key SSH access to the machine of
anyone who ran it, and no later tooling would have caught that — it is not
a secret, it is a behaviour.

## 1. Credentials of a known shape, over all history

```bash
git grep -I -n -E \
  'ghp_[A-Za-z0-9]{20}|github_pat_[A-Za-z0-9_]{20}|gh[osu]_[A-Za-z0-9]{20}|BEGIN (RSA |EC |OPENSSH |)PRIVATE KEY|AKIA[0-9A-Z]{16}|xox[baprs]-[0-9A-Za-z-]{10}|AIza[0-9A-Za-z_-]{30}' \
  $(git rev-list --all)
```

Over all commits, not the working tree: a secret deleted in a later commit
is still published. Check the repo is small enough for this to be
exhaustive (`git count-objects -vH`); if it is not, go straight to step 5.

## 2. Credentials that do **not** look like credentials

The greps above only find secrets shaped like secrets. These two found the
ones that mattered:

```bash
grep -rn -iE 'unlock-keychain|password|passphrase|secret|token' \
  --include='*.sh' --include='*.ps1' --include='*.rs' . | grep -v /target/
grep -rn -E 'ssh-ed25519 AAAA|ssh-rsa AAAA|ecdsa-sha2' . | grep -v /target/
```

**A public key is the dangerous case nobody expects.** Publishing it grants
nothing; a script that *authorizes* it on the machine running the script
grants everything. Read every hit and ask "whose machine does this change,
and who chose the key?" ([[../rules/no-secrets-in-source]]).

## 3. Files named like credentials

```bash
git log --all --diff-filter=A --name-only --pretty=format: | sort -u |
  grep -iE 'cred|secret|token|\.pem$|\.key$|\.p12$|\.pfx$|id_rsa|id_ed25519|\.env|keychain'
```

Hits are usually source files *about* credentials rather than credentials.
Open each one anyway.

## 4. Personal data, counted so the work can be planned

```bash
for p in <hostname> <user> <home path> <ssh alias>; do
  printf '%-24s %3s files\n' "$p" "$(grep -rIl --exclude-dir=target --exclude-dir=.git -F "$p" . | wc -l)"
done
```

Then split the hits: **specs and docs may keep them** as evidence under a
stable alias; **code, defaults and scripts may not**, because a default is
inherited by the next user ([[../rules/no-personal-data]]). A file that is
personal all the way down gets deleted, not anonymized.

## 5. A real scanner, over the full history

```bash
gitleaks detect --source . --log-opts '--all'     # or
trufflehog git file://. --since-commit $(git rev-list --max-parents=0 HEAD)
```

Attach the report to the PR that flips visibility. Steps 1–4 are a floor,
not a ceiling.

## 6. Leakage from unrelated work

Grep for the names of every other client, product and credential store the
team works with. Expect false positives and resolve them before trusting a
count: a short name also matches inside longer words, and one such
search returned 32 files without a single real hit.

## 7. The workflows

- No `pull_request_target`. It runs with the base repository's token.
- Any job that touches a secret must be unreachable from a fork.
- A release or publish job stays gated on the tag ref.
- Check which secrets the workflows name: `grep -rn 'secrets\.' .github/`.

## 8. The legal minimum, or nobody can contribute

`LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`. A public repository with no
license is "all rights reserved", which defeats the purpose of publishing
it.

## 9. Then flip, and only then

Turn on **secret scanning** and **push protection** immediately after.
Both are free on public repositories, and they are the only part of this
procedure that keeps working without anyone remembering to run it.

## Rotate, don't rewrite

If the audit finds a credential already committed, prefer rotating it: a
rotated value makes every published copy worthless and the history stays
citable. A history rewrite is for a secret that cannot be rotated, and it
breaks every commit hash the specs cite as evidence.

## See also

- [[../rules/no-secrets-in-source]], [[../rules/no-personal-data]]
- [[../../specs/018-open-source]] — the findings of the first run, and the
  argument against each step
