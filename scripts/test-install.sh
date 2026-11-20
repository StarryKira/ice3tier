#!/bin/sh
# Runs deploy/install.sh for real on a throwaway machine (CI): installs the node from DIR as a system service (systemd
# on Linux, launchd on macOS), joins it to a controller started here on loopback with `sudo i3t join`, checks that it
# comes online, that `sudo i3t run` points at the service and that asking for help does not restart it, installs over
# it, then uninstalls and checks nothing is left. Needs passwordless sudo and changes the machine: do not run it on
# yours.
#
# Usage: scripts/test-install.sh <dir with i3t, i3t-controller and i3tctl>
set -eu

dir=$(cd "${1:?usage: test-install.sh <dir with i3t, i3t-controller and i3tctl>}" && pwd)
repo=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
os=$(uname -s)
ctl_pid=""
label="com.github.fuqiuluo.ice3tier"

finish() {
    status=$?
    if [ "$status" -ne 0 ]; then
        printf '\n--- failed; the node says:\n'
        if [ "$os" = "Linux" ]; then
            sudo journalctl -u i3t --no-pager -n 80 || true
        else
            sudo tail -n 80 /var/log/i3t.log || true
        fi
        printf -- '--- the controller says:\n'
        if [ -n "$ctl_pid" ] || [ "$os" != "Linux" ]; then
            tail -n 40 "$work/ctl.err" || true
        else
            sudo journalctl -u i3t-controller --no-pager -n 40 || true
        fi
    fi
    if [ -n "$ctl_pid" ]; then
        kill "$ctl_pid" 2>/dev/null || true
    fi
    rm -rf "$work"
    exit "$status"
}
trap finish EXIT

step() {
    printf '\n=== %s\n' "$*"
}

fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

ctl() {
    "$dir/i3tctl" --json "$@"
}

all_online() {
    ctl node list | python3 -I -c 'import json, sys; nodes = json.load(sys.stdin); sys.exit(0 if nodes and all(n["online"] for n in nodes) else 1)'
}

service_running() {
    if [ "$os" = "Linux" ]; then
        systemctl is-active -q i3t
    else
        sudo launchctl print "system/$label" 2>/dev/null | grep -q 'state = running'
    fi
}

wait_online() {
    i=0
    until service_running && all_online; do
        i=$((i + 1))
        [ "$i" -lt 60 ] || fail "the node did not come online within 60 s"
        sleep 1
    done
}

# `sudo ARGS` must point at the service instead of starting a node.
points_at_service() {
    if sudo "$@" 2>"$work/run.err"; then fail "sudo $* started a node"; fi
    grep -q "system service" "$work/run.err" || fail "sudo $*: unexpected answer: $(cat "$work/run.err")"
}

step "controller on loopback"
"$dir/i3t-controller" --data-dir "$work/ctl" --public-addr 127.0.0.1:20331 --listen 127.0.0.1:20331 >"$work/ctl.out" 2>"$work/ctl.err" &
ctl_pid=$!
i=0
until grep -q '^    [^ ]' "$work/ctl.out" 2>/dev/null; do
    i=$((i + 1))
    [ "$i" -lt 60 ] || fail "the controller printed no admin token"
    sleep 1
done
I3T_ADMIN_TOKEN=$(sed -n 's/^    \([^ ]*\)$/\1/p' "$work/ctl.out" | head -n 1)
I3T_ADMIN_URL="http://127.0.0.1:20332"
export I3T_ADMIN_TOKEN I3T_ADMIN_URL
ctl network create ci >/dev/null
token=$(ctl token create --expires 30m | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["token"])')

step "install without a token"
sudo sh "$repo/deploy/install.sh" --from "$dir"
if [ "$os" = "Linux" ]; then
    [ "$(systemctl is-enabled i3t)" = "enabled" ] || fail "i3t.service is not enabled"
    if systemctl is-active -q i3t; then fail "i3t.service runs before joining"; fi
else
    [ -f "/Library/LaunchDaemons/$label.plist" ] || fail "no launchd plist"
fi

step "sudo i3t run points at the service"
points_at_service i3t run
points_at_service i3t --state-dir /var/lib/i3t/ run
points_at_service env I3T_STATE_DIR="$work/elsewhere" i3t --state-dir /var/lib/i3t run

step "sudo i3t join joins and starts the service"
printf '%s\n' "$token" | sudo i3t join --can-relay
wait_online
sudo i3t status
if [ "$os" = "Linux" ]; then
    pid=$(systemctl show -p MainPID --value i3t)
    [ "$(ps -o user= -p "$pid" | tr -d ' ')" = "i3t" ] || fail "the node does not run as user i3t"
fi

step "asking for help leaves the service alone"
before=$(pgrep -x i3t) || fail "no node process"
sudo i3t join --help >/dev/null
sudo i3t --help join >/dev/null
sudo i3t run --help >/dev/null
[ "$(pgrep -x i3t)" = "$before" ] || fail "asking for help restarted the service"

step "install over the joined node"
sudo sh "$repo/deploy/install.sh" --from "$dir"
sleep 3
wait_online
if [ "$os" = "Darwin" ]; then
    step "install over the joined node with a changed launchd plist"
    printf '<!-- old -->\n' | sudo tee -a "/Library/LaunchDaemons/$label.plist" >/dev/null
    sudo sh "$repo/deploy/install.sh" --from "$dir"
    if grep -q '<!-- old -->' "/Library/LaunchDaemons/$label.plist"; then fail "launchd plist was not replaced"; fi
    sleep 3
    wait_online
fi

step "uninstall --purge"
sudo sh "$repo/deploy/install.sh" --uninstall --purge
for path in /usr/local/bin/i3t /var/lib/i3t /etc/systemd/system/i3t.service "/Library/LaunchDaemons/$label.plist"; do
    [ ! -e "$path" ] || fail "$path is still there"
done
if pgrep -x i3t >/dev/null; then fail "an i3t process is still running"; fi

if [ "$os" = "Linux" ]; then
    step "install controller and print its first-start admin token across TZ"
    kill "$ctl_pid" 2>/dev/null || true
    wait "$ctl_pid" 2>/dev/null || true
    ctl_pid=""
    TZ=Asia/Shanghai sh "$repo/deploy/install.sh" --from "$dir" --controller --public-addr 127.0.0.1:20331 >"$work/ctl-install.out"
    cat "$work/ctl-install.out"
    grep -q '^    [^ ]' "$work/ctl-install.out" || fail "controller install did not print the initial admin token"
    systemctl is-active -q i3t-controller || fail "i3t-controller.service is not active"

    step "uninstall controller --purge"
    sudo sh "$repo/deploy/install.sh" --uninstall --controller --purge
    for path in /usr/local/bin/i3t-controller /usr/local/bin/i3tctl /var/lib/i3t-controller /etc/i3t /etc/systemd/system/i3t-controller.service; do
        [ ! -e "$path" ] || fail "$path is still there"
    done
fi

step "ok"
