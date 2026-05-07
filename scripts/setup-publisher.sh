#!/usr/bin/env bash
# One-time bootstrap for the dedicated `publisher` user that creates draft
# GitHub PRs from accepted optimization sessions. Linux-only; must be run as
# root (or via sudo). Idempotent — safe to re-run.
#
# What it does
#   1. Creates the publisher system user (no login shell, no password).
#   2. Creates the token directory and an empty token placeholder, both
#      owned by publisher:publisher (0700 / 0600).
#   3. Drops a sudoers stanza allowing the agent user to invoke
#      `publish-accepted.sh` as the publisher with NOPASSWD, and ONLY that
#      script. Validates with `visudo -cf` before installing.
#   4. Prints final instructions for placing the GitHub PAT.
#
# What it does NOT do
#   - Make $FRAMEWORK_ROOT, $AGENTIC_ENV_FILE, or the sessions tree readable
#     by the publisher. The operator is responsible for that — typically by
#     adding the publisher to a shared group and ensuring those paths are
#     group-readable. Phase 5 will fail with a clear error from publish-accepted.sh
#     if the publisher cannot read .env or BASE.
#   - Configure git's safe.directory for the publisher. If you see
#     `fatal: detected dubious ownership` from publish-accepted.sh, run:
#       sudo -H -u $PUBLISH_SUDO_USER git config --global \
#         --add safe.directory $FRAMEWORK_ROOT/repos/stacks-core
#     `scripts/preflight.sh` exercises this path under the publisher when
#     PUBLISH_ACCEPTED_PRS=1, so it will catch the issue before a real run.
#
# Defaults can be overridden via env vars:
#   PUBLISH_SUDO_USER     (default: publisher)
#   AGENT_USER            (default: $SUDO_USER, fallback: agent)
#   PUBLISH_TOKEN_FILE    (default: /var/lib/stacks-core-agentic/gh_token)
#   FRAMEWORK_ROOT        (default: parent dir of this script's dir)
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "setup-publisher: must run as root (try: sudo $0)" >&2
  exit 1
fi

PUBLISH_SUDO_USER="${PUBLISH_SUDO_USER:-publisher}"
AGENT_USER="${AGENT_USER:-${SUDO_USER:-agent}}"
PUBLISH_TOKEN_FILE="${PUBLISH_TOKEN_FILE:-/var/lib/stacks-core-agentic/gh_token}"
FRAMEWORK_ROOT="${FRAMEWORK_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
PUBLISH_SCRIPT="$FRAMEWORK_ROOT/scripts/publish-accepted.sh"

if ! id -u "$AGENT_USER" >/dev/null 2>&1; then
  echo "setup-publisher: agent user '$AGENT_USER' does not exist" >&2
  exit 1
fi

if [ ! -x "$PUBLISH_SCRIPT" ]; then
  echo "setup-publisher: publish script not executable: $PUBLISH_SCRIPT" >&2
  exit 1
fi

# 1. Create the publisher user if missing.
if id -u "$PUBLISH_SUDO_USER" >/dev/null 2>&1; then
  echo "publisher user '$PUBLISH_SUDO_USER' already exists; reusing"
else
  useradd --system --create-home --shell /usr/sbin/nologin "$PUBLISH_SUDO_USER"
  echo "created publisher user '$PUBLISH_SUDO_USER'"
fi

# 2. Token directory and (placeholder) file.
TOKEN_DIR="$(dirname "$PUBLISH_TOKEN_FILE")"
mkdir -p "$TOKEN_DIR"
chown "$PUBLISH_SUDO_USER:$PUBLISH_SUDO_USER" "$TOKEN_DIR"
chmod 0700 "$TOKEN_DIR"
if [ ! -e "$PUBLISH_TOKEN_FILE" ]; then
  install -m 0600 -o "$PUBLISH_SUDO_USER" -g "$PUBLISH_SUDO_USER" /dev/null "$PUBLISH_TOKEN_FILE"
  echo "created empty token file: $PUBLISH_TOKEN_FILE"
fi

# 3. Sudoers stanza. Validated before install via a temp file + `visudo -cf`.
SUDOERS_FILE="/etc/sudoers.d/stacks-core-agentic-publisher"
TMP_SUDOERS="$(mktemp)"
trap 'rm -f "$TMP_SUDOERS"' EXIT

cat >"$TMP_SUDOERS" <<EOF
# Created by scripts/setup-publisher.sh. Do not edit by hand; re-run the
# script if AGENT_USER, PUBLISH_SUDO_USER, or the script path changes.
$AGENT_USER ALL=($PUBLISH_SUDO_USER) NOPASSWD: $PUBLISH_SCRIPT
Defaults:$AGENT_USER env_keep += "OPT_SESSION_DIR FRAMEWORK_ROOT AGENTIC_ENV_FILE"
EOF

if ! visudo -cf "$TMP_SUDOERS" >/dev/null; then
  echo "setup-publisher: generated sudoers file failed visudo validation" >&2
  exit 1
fi

install -m 0440 -o root -g root "$TMP_SUDOERS" "$SUDOERS_FILE"
echo "installed sudoers: $SUDOERS_FILE"

echo
echo "Publisher setup complete."
echo
echo "Final manual step:"
echo "  Place a GitHub PAT (scope: 'repo') into:"
echo "    $PUBLISH_TOKEN_FILE"
echo "  with mode 0600 owned $PUBLISH_SUDO_USER:$PUBLISH_SUDO_USER. Example:"
echo "    sudo install -m 0600 -o $PUBLISH_SUDO_USER -g $PUBLISH_SUDO_USER /tmp/token $PUBLISH_TOKEN_FILE"
echo
echo "Verify the setup by running scripts/preflight.sh as the agent user with"
echo "PUBLISH_ACCEPTED_PRS=1 in your .env. It will check sudoers, the token,"
echo "and that gh is available to the publisher. Manual checks if you prefer:"
echo
echo "  # Token is non-empty and readable by publisher (publish script reads it):"
echo "  sudo -nH -u $PUBLISH_SUDO_USER test -s $PUBLISH_TOKEN_FILE && echo OK"
echo
echo "  # NOPASSWD sudo wiring works:"
echo "  sudo -nH -u $PUBLISH_SUDO_USER true && echo OK"
echo
echo "  # gh CLI is available to the publisher:"
echo "  sudo -nH -u $PUBLISH_SUDO_USER gh --version"
