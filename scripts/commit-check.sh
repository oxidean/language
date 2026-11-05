#!/bin/sh
# commit-check — commit-message gate for oxl.
#
# Two contracts, enforced on every surface (hook, CI, ad-hoc):
#
#   1. SemVer-prefix grammar on the subject line (same shape as the
#      oxidean monorepo's conventional-commit contract):
#          type(scope)?(!)? ": " subject
#      types: feat fix perf revert docs chore test style refactor build ci
#      exempt: "Merge ...", "Revert \"...\"", "chore(release): ..."
#
#   2. No AI/tool attribution or markers anywhere in the message
#      (.cursor/rules/no-ai-attribution.mdc): no AI Co-Authored-By /
#      Generated-with / Made-with trailers, no bot signatures, no
#      zero-width or invisible markers. Human co-authors and
#      Signed-off-by stay welcome.
#
# Usage:
#   commit-check.sh --file <msgfile>     commit-msg hook entry point
#   commit-check.sh --message "<msg>"    lint one message
#   commit-check.sh --pr-title "<t>"     lint a PR title
#   commit-check.sh --range A..B         lint every commit in a range
#   commit-check.sh --scan-file <f>      attribution scan only (PR body)
#
# Exit 0 clean, 1 on any violation. Warnings print but do not fail.
set -eu

TYPES='feat|fix|perf|revert|docs|chore|test|style|refactor|build|ci'
SUBJECT_WARN=100
fail=0

err() { printf '%s\n' "commit-check: $*" >&2; fail=1; }
warn() { printf '%s\n' "commit-check: warning: $*" >&2; }

# --- attribution scan: whole message text -----------------------------

check_attribution() {
    _msg=$1
    # AI co-author / bot trailers. A human Co-authored-by is fine; the
    # ban is on AI vendors, bots, and *-ai-* / [bot] addresses.
    if printf '%s\n' "$_msg" | grep -iE '^[[:space:]]*co-?authored-by:' \
        | grep -qiE 'claude|cursor|copilot|chatgpt|codex|gemini|devin|openai|anthropic|codeium|windsurf|swe-|\[bot\]|[a-z0-9-]*bot@|ai-integration'; then
        err "AI Co-authored-by trailer"
    fi
    # generator footers: "Generated with <tool>", "Made with: X",
    # "Authored via X" — the tool names that sign output, plus the
    # trailer forms regardless of vendor.
    if printf '%s\n' "$_msg" | grep -iqE '(generated|made|created|authored|written|produced)[[:space:]]+(with|by|via)[[:space:]]*\[?[[:space:]]*(claude|cursor|copilot|chatgpt|codex|gemini|devin|openai|anthropic|codeium|windsurf|swe-)'; then
        err "AI generator footer"
    fi
    if printf '%s\n' "$_msg" | grep -iqE '^[[:space:]]*(generated|made|authored)[- ](with|by|via)[[:space:]]*:'; then
        err "generator trailer"
    fi
    # bot-emoji generator signature
    if printf '%s\n' "$_msg" | grep -q '🤖'; then
        err "bot-emoji attribution marker"
    fi
    # AI named as the author/agent — not as the subject of the change.
    # (kept narrow on purpose: "add Copilot webhook" must stay legal)
    if printf '%s\n' "$_msg" | grep -iqE '\b(as an ai|i generated this|authored by (claude|cursor|copilot|chatgpt|codex|gemini|devin|openai|swe-))\b'; then
        err "AI self-attribution"
    fi
    # vendor-stamped HTML comments smuggled into the message
    if printf '%s\n' "$_msg" | grep -iqE '<!--[^>]*(claude|cursor|copilot|chatgpt|codex|gemini|devin|openai|anthropic|generated|🤖)'; then
        err "attribution HTML comment"
    fi
    # zero-width / invisible markers: U+200B..D, U+2060, U+FEFF as raw
    # UTF-8 byte sequences — byte grep, no -P dependency
    if printf '%s\n' "$_msg" | LC_ALL=C grep -q \
        -e "$(printf '\342\200\213')" -e "$(printf '\342\200\214')" \
        -e "$(printf '\342\200\215')" -e "$(printf '\342\201\240')" \
        -e "$(printf '\357\273\277')"; then
        err "zero-width/invisible attribution marker"
    fi
}

# --- grammar: subject line only ---------------------------------------

check_subject() {
    _subject=$1
    _where=${2:-subject}
    # exempt shapes: merges, reverts, release tooling commits
    case "$_subject" in
        "Merge "*|"Revert \""*|"chore(release):"*) return 0 ;;
    esac
    if ! printf '%s\n' "$_subject" | grep -qE '^[A-Za-z]+(\([^)]+\))?!?: +[^ ]'; then
        err "$_where does not match \"type(scope)!: subject\": ${_subject}"
        return
    fi
    _type=$(printf '%s\n' "$_subject" | sed -E 's/^([A-Za-z]+).*/\1/')
    if ! printf '%s\n' "$_type" | grep -qxE "$TYPES"; then
        err "$_where uses unknown type \"$_type\" (allowed: $(printf '%s' "$TYPES" | tr '|' ' '))"
    fi
    if [ "${#_subject}" -gt "$SUBJECT_WARN" ]; then
        warn "$_where is ${#_subject} chars (> $SUBJECT_WARN)"
    fi
}

check_message() {
    _msg=$1
    _where=${2:-subject}
    check_subject "$(printf '%s\n' "$_msg" | head -1)" "$_where"
    check_attribution "$_msg"
}

# --- modes -------------------------------------------------------------

case "${1:-}" in
    --file)
        [ -f "${2:-}" ] || { err "--file needs a readable message file"; exit 1; }
        check_message "$(cat "$2")" "subject"
        ;;
    --message)
        check_message "${2:-}" "subject"
        ;;
    --pr-title)
        check_message "${2:-}" "PR title"
        ;;
    --scan-file)
        [ -f "${2:-}" ] || { err "--scan-file needs a readable file"; exit 1; }
        check_attribution "$(cat "$2")"
        ;;
    --range)
        _range=${2:-}
        [ -n "$_range" ] || { err "--range needs A..B"; exit 1; }
        for _sha in $(git rev-list "$_range"); do
            _msg=$(git log -1 --format=%B "$_sha")
            _before=$fail
            check_message "$_msg" "commit ${_sha}"
            [ "$fail" = "$_before" ] || printf '%s\n' "  ^^ in range $_range" >&2
        done
        ;;
    *)
        sed -n '2,33p' "$0" >&2
        exit 2
        ;;
esac
exit "$fail"
