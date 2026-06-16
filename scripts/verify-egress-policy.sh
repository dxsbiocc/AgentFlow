#!/usr/bin/env bash
#
# Smoke-test an AgentFlow deployment egress policy from inside the isolated
# runtime namespace/container.
#
# Usage:
#   scripts/verify-egress-policy.sh
#
# Optional environment:
#   AF_EGRESS_VERIFY_ASSUME_ISOLATED=1
#     Force probes to run even if Docker/netns indicators are not detected.
#   AF_EGRESS_VERIFY_PUBLIC_URL=https://www.cbioportal.org/
#     Public HTTPS allowlist target to probe.
#   AF_EGRESS_VERIFY_RESOLVE=www.cbioportal.org:443:203.0.113.10
#     curl-style host:port:addr override for IP-snapshot allowlists.
#   AF_EGRESS_VERIFY_TIMEOUT=3
#     Per-connect timeout in seconds.
#   AF_EGRESS_VERIFY_ALLOW_OFFLINE=1
#     Treat public target failure as a skip after blocked probes pass.
#
# This script does not configure networking and does not require root. If it is
# not running in a detected container/network namespace, it prints a message and
# exits 0 so developer machines are not misclassified as policy failures.

set -euo pipefail

PUBLIC_URL="${AF_EGRESS_VERIFY_PUBLIC_URL:-https://www.cbioportal.org/}"
CONNECT_TIMEOUT="${AF_EGRESS_VERIFY_TIMEOUT:-3}"

log() {
  printf '%s\n' "$*"
}

is_linux() {
  [ "$(uname -s 2>/dev/null || true)" = "Linux" ]
}

netns_differs_from_pid1() {
  [ -e /proc/self/ns/net ] || return 1
  [ -e /proc/1/ns/net ] || return 1
  [ "$(readlink /proc/self/ns/net)" != "$(readlink /proc/1/ns/net)" ]
}

in_container() {
  [ -f /.dockerenv ] && return 0
  [ -f /run/.containerenv ] && return 0
  grep -qaE '(docker|containerd|kubepods|libpod|podman)' \
    /proc/1/cgroup /proc/self/cgroup 2>/dev/null
}

in_isolated_environment() {
  [ "${AF_EGRESS_VERIFY_ASSUME_ISOLATED:-0}" = "1" ] && return 0
  is_linux || return 1
  in_container && return 0
  netns_differs_from_pid1 && return 0
  return 1
}

have_default_route() {
  if command -v ip >/dev/null 2>&1; then
    ip route show default 2>/dev/null | grep -q .
    return
  fi

  if [ -r /proc/net/route ]; then
    awk '$2 == "00000000" { found = 1 } END { exit found ? 0 : 1 }' /proc/net/route
    return
  fi

  return 1
}

require_python() {
  if ! command -v python3 >/dev/null 2>&1; then
    log "[fail] python3 is required for egress probes inside an isolated environment"
    exit 1
  fi
}

probe_blocked() {
  local label="$1"
  local host="$2"
  local port="$3"

  if python3 - "$host" "$port" "$CONNECT_TIMEOUT" <<'PY'
import socket
import sys

host = sys.argv[1]
port = int(sys.argv[2])
timeout = float(sys.argv[3])

try:
    sock = socket.create_connection((host, port), timeout=timeout)
except OSError as exc:
    print(f"blocked/unreachable: {host}:{port} ({exc.__class__.__name__}: {exc})")
    sys.exit(0)
else:
    sock.close()
    print(f"reachable: {host}:{port}")
    sys.exit(1)
PY
  then
    log "[ok] $label probe is blocked or unreachable ($host:$port)"
    return 0
  fi

  log "[fail] $label probe unexpectedly reached $host:$port"
  return 1
}

probe_public_with_curl() {
  local resolve_args=()

  if [ -n "${AF_EGRESS_VERIFY_RESOLVE:-}" ]; then
    resolve_args=(--resolve "$AF_EGRESS_VERIFY_RESOLVE")
  fi

  curl -sSI \
    --connect-timeout "$CONNECT_TIMEOUT" \
    --max-time "$((CONNECT_TIMEOUT + 7))" \
    "${resolve_args[@]}" \
    -o /dev/null \
    "$PUBLIC_URL"
}

probe_public_with_python() {
  python3 - "$PUBLIC_URL" "$CONNECT_TIMEOUT" <<'PY'
import os
import socket
import ssl
import sys
from urllib.parse import urlparse

url = sys.argv[1]
timeout = float(sys.argv[2])
parsed = urlparse(url)

if parsed.scheme != "https" or not parsed.hostname:
    print(f"unsupported public probe URL: {url}", file=sys.stderr)
    sys.exit(1)

host = parsed.hostname
port = parsed.port or 443
connect_host = host
resolve = os.environ.get("AF_EGRESS_VERIFY_RESOLVE", "")

if resolve:
    parts = resolve.rsplit(":", 2)
    if len(parts) == 3 and parts[0] == host and int(parts[1]) == port:
        connect_host = parts[2]

path = parsed.path or "/"
if parsed.query:
    path += "?" + parsed.query

request = (
    f"HEAD {path} HTTP/1.1\r\n"
    f"Host: {host}\r\n"
    "User-Agent: agentflow-egress-verify/1\r\n"
    "Connection: close\r\n\r\n"
).encode("ascii")

context = ssl.create_default_context()

try:
    with socket.create_connection((connect_host, port), timeout=timeout) as raw:
        with context.wrap_socket(raw, server_hostname=host) as tls:
            tls.settimeout(timeout)
            tls.sendall(request)
            response = tls.recv(128)
except OSError as exc:
    print(f"public probe failed: {exc.__class__.__name__}: {exc}", file=sys.stderr)
    sys.exit(1)

if response.startswith(b"HTTP/"):
    sys.exit(0)

print("public probe did not receive an HTTP response", file=sys.stderr)
sys.exit(1)
PY
}

probe_public() {
  if command -v curl >/dev/null 2>&1; then
    probe_public_with_curl
    return
  fi

  probe_public_with_python
}

main() {
  if ! in_isolated_environment; then
    log "[skip] This script should run inside one of the isolated recipe environments."
    log "[skip] No container or separate Linux network namespace was detected; exiting 0."
    exit 0
  fi

  require_python

  local failures=0

  probe_blocked "metadata" "169.254.169.254" "80" || failures=$((failures + 1))
  probe_blocked "RFC1918" "10.0.0.1" "80" || failures=$((failures + 1))
  probe_blocked "loopback" "127.0.0.1" "80" || failures=$((failures + 1))
  probe_blocked "CGNAT" "100.64.0.1" "80" || failures=$((failures + 1))

  if [ "$failures" -ne 0 ]; then
    log "[fail] $failures blocked-destination probe(s) unexpectedly reached their target"
    exit 1
  fi

  if probe_public; then
    log "[ok] public allowlist target is reachable: $PUBLIC_URL"
    exit 0
  fi

  if ! have_default_route; then
    log "[skip] public allowlist target is not reachable because no default route is present"
    log "[skip] blocked-destination probes passed; treating this as a no-network baseline"
    exit 0
  fi

  if [ "${AF_EGRESS_VERIFY_ALLOW_OFFLINE:-0}" = "1" ]; then
    log "[skip] public allowlist target is not reachable; AF_EGRESS_VERIFY_ALLOW_OFFLINE=1"
    log "[skip] blocked-destination probes passed"
    exit 0
  fi

  log "[fail] public allowlist target is not reachable despite an apparent default route: $PUBLIC_URL"
  exit 1
}

main "$@"
