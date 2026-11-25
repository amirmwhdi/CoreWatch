#!/bin/sh
# Run the checks before every `git push`, now that GitHub no longer does.
#
#   tests/install-hook.sh           before each push: tests/run.sh (this machine, a minute or two)
#   tests/install-hook.sh --full    before each push: tests/ci.sh (every distribution and the Flatpak; slow)
#   tests/install-hook.sh --remove  take the hook away
#
# A push that fails the checks is stopped. To push anyway, once:
# git push --no-verify
set -eu

cd "$(dirname "$0")/.." || exit 1

hooks=$(git rev-parse --git-path hooks)
hook="$hooks/pre-push"
marker="# Installed by tests/install-hook.sh"

case ${1:-} in
    "") command="tests/run.sh" ;;
    --full) command="tests/ci.sh" ;;
    --remove)
        if [ -f "$hook" ] && grep -q "$marker" "$hook"; then
            rm -f "$hook"
            echo "Removed $hook"
        else
            echo "No hook from tests/install-hook.sh to remove."
        fi
        exit 0
        ;;
    -h | --help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install-hook.sh: unknown argument: $1" >&2; exit 2 ;;
esac

if [ -f "$hook" ] && ! grep -q "$marker" "$hook"; then
    echo "install-hook.sh: $hook exists and is not ours; leaving it alone." >&2
    exit 1
fi

mkdir -p "$hooks"
cat >"$hook" <<EOF
#!/bin/sh
$marker: the checks CI used to run.
# Push anyway, once: git push --no-verify. Remove: tests/install-hook.sh --remove
cd "\$(git rev-parse --show-toplevel)" || exit 1
echo "pre-push: $command (skip once with git push --no-verify)"
exec $command
EOF
chmod +x "$hook"
echo "Installed: every git push now runs $command first."
