#!/usr/bin/env bash
#
# Regenerates media/preview.png: the branch picker for the fictional repositories from
# kiosk's preview fixture, with some of their checkouts open in Rex. A stand-in `rex`
# answers the picker's questions about the Rex server, so no Rex server is involved.
# The fixture and renderer come from kiosk's flake, at the revision below unless
# KIOSK_FLAKE names another, such as a local checkout.
#
# Usage, from anywhere in the repository:
#   scripts/preview.sh

set -euo pipefail

REPO_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
readonly REPO_DIR
readonly OUTPUT="$REPO_DIR/media/preview.png"
readonly KIOSK_FLAKE=${KIOSK_FLAKE:-github:thomasschafer/kiosk/22f95011adcae88c31a380d39300aac876d89eec}

for tool in cargo nix; do
    command -v "$tool" >/dev/null || {
        printf '%s is not on PATH\n' "$tool" >&2
        exit 1
    }
done

TEMP=$(mktemp -d "${TMPDIR:-/tmp}/rex-kiosk-preview.XXXXXX")
TEMP=$(cd "$TEMP" && pwd -P)
readonly TEMP ROOT="$TEMP/fixture"
trap 'rm -rf -- "$TEMP"' EXIT

printf 'Building rex-kiosk...\n'
cargo build --quiet --locked --release --manifest-path "$REPO_DIR/Cargo.toml"

nix run "$KIOSK_FLAKE#preview-fixture" -- "$ROOT"
readonly TIDEPOOL="$ROOT/repos/tidepool" WORKTREES="$ROOT/worktrees/tidepool"

mkdir -p "$TEMP/bin" "$TEMP/config/rex-kiosk" "$TEMP/state"
cat >"$TEMP/config/rex-kiosk/config.toml" <<EOF
search_dirs = ["$ROOT/repos"]

[worktrees]
directory = "$ROOT/worktrees"
EOF

# One session, tidepool, with a window for each of three checkouts; the picker floats
# over the first. Any other request fails, so a picker that asks for more stops the run.
cat >"$TEMP/bin/rex" <<EOF
#!/bin/sh
set -eu
terminal() {
    printf '{"block_id":"block:%s","creator_name":"com.superlogical.terminal","host_id":"host:demo"}' "\$1"
}
window() {
    printf '{"window_id":"window:%s","layers":[{"kind":"tiled","blocks":[%s]}%s]}' \\
        "\$1" "\$(terminal "\$1")" "\${2:-}"
}
case "\$*" in
    "api call server.status {}")
        echo '{"host_id":"host:demo"}' ;;
    "api call session.list {}")
        echo '{"sessions":[{"session_id":"session:tidepool"}]}' ;;
    'api call session.view {"session_id":"session:tidepool"}')
        printf '{"label":"tidepool","windows":[%s,%s,%s]}\n' \\
            "\$(window main ',{"kind":"floating","blocks":[{"block_id":"block:picker"}]}')" \\
            "\$(window offline-sync)" "\$(window timezone-rounding)" ;;
    *'com.superlogical.terminal.process'*'"block_id":"block:main"'*)
        echo '{"foreground":{"cwd":"$TIDEPOOL"}}' ;;
    *'com.superlogical.terminal.process'*'"block_id":"block:offline-sync"'*)
        echo '{"foreground":{"cwd":"$WORKTREES/feat-offline-sync"}}' ;;
    *'com.superlogical.terminal.process'*'"block_id":"block:timezone-rounding"'*)
        echo '{"foreground":{"cwd":"$WORKTREES/fix-timezone-rounding"}}' ;;
    *)
        echo "Error: the preview's stand-in rex cannot answer: \$* (status=501 code=not_implemented)" >&2
        exit 1 ;;
esac
EOF
chmod +x "$TEMP/bin/rex"

printf 'Rendering the picker...\n'
nix run "$KIOSK_FLAKE#preview-render" -- \
    --cols 108 --rows 30 --output "$OUTPUT" --cwd "$TIDEPOOL" \
    --wait "select repo" --send Tab --wait "select branch" \
    --wait origin/refactor/sync-queue --wait open \
    --send Down --send Down \
    -- env \
    HOME="$TEMP" \
    XDG_CONFIG_HOME="$TEMP/config" \
    XDG_STATE_HOME="$TEMP/state" \
    REX_BIN_DIR="$TEMP/bin" \
    REX_SESSION=session:tidepool \
    REX_BLOCK=block:picker \
    "$REPO_DIR/target/release/rex-kiosk"
