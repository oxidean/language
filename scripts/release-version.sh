#!/bin/sh
# Compute the next release tag from conventional commit subjects.
#
#   release-version.sh latest          highest vX.Y.Z tag (semver order); 0.0.0 if none
#   release-version.sh bump            read `git log --format=%H` SHAs on stdin -> major|minor|patch
#   release-version.sh bump-msg        read one commit message on stdin   -> major|minor|patch
#   release-version.sh next [RANGE]    latest tag + bump over RANGE (default HEAD) -> vX.Y.Z
#
# Bump rules (commit-check.sh grammar: type(scope)!: subject):
#   '!' marker or BREAKING CHANGE/-CHANGE footer -> major (minor while major==0)
#   feat                                          -> minor
#   everything else                               -> patch
set -eu

cmd=${1:-next}

bump_msg() { # commit message on stdin
    msg=$(cat)
    subj=$(printf '%s\n' "$msg" | sed -n '1{s/\r$//;p;q}')
    if printf '%s' "$subj" | grep -qE '^[A-Za-z]+(\([^)]*\))?!:' \
    || printf '%s' "$msg" | grep -qiE '^BREAKING[- ]CHANGE:'; then
        echo major; return
    fi
    case "$subj" in feat:*|feat\(*\):*) echo minor;; *) echo patch;; esac
}

rank() { case "$1" in major) echo 3;; minor) echo 2;; *) echo 1;; esac; }

case "$cmd" in
bump-msg)
    bump_msg
    ;;
bump)
    best="patch"
    while IFS= read -r sha; do
        [ -n "$sha" ] || continue
        msg=$(git log -1 --format=%B "$sha")
        b=$(printf '%s\n' "$msg" | bump_msg)
        [ "$(rank "$b")" -gt "$(rank "$best")" ] && best=$b
    done
    echo "$best"
    ;;
latest)
    tags=$(git tag -l 'v*')
    printf '%s\n' "$tags" | grep -xE 'v[0-9]+\.[0-9]+\.[0-9]+' | sort -V | tail -1 | sed 's/^v//' || true
    ;;
next)
    range=${2:-HEAD}
    latest=$("$0" latest); latest=${latest:-0.0.0}
    IFS=. read -r M m p <<EOF2
$latest
EOF2
    shas=$(git log --format=%H "$range")
    bump=$(printf '%s\n' "$shas" | "$0" bump)
    case "$bump" in
        major) if [ "$M" -eq 0 ]; then m=$((m+1)); p=0; else M=$((M+1)); m=0; p=0; fi;;
        minor) m=$((m+1)); p=0;;
        *)     p=$((p+1));;
    esac
    echo "v$M.$m.$p"
    ;;
*)
    echo "usage: $0 {latest|bump|bump-msg|next [RANGE]}" >&2; exit 2
    ;;
esac
