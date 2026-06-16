# Deployment Egress Containment Recipes

Status: issue #36 deployment-level recipe
Scope: Linux deployment environments for generated tool verification/runtime

AgentFlow already has a Python in-process egress guard for generated tools. That
guard is useful defense-in-depth: it fails early with readable errors for
cooperative scripts and common prompt-injection paths. It is not anti-tamper
containment. A generated Python program can reassign patched socket functions,
replace imports, use a native extension, or invoke another interpreter.

If the threat model includes an active anti-tamper script, the egress policy
must live outside the process, at an OS or runtime boundary. Use one of the
Linux recipes below for that boundary, and keep the in-process guard only as an
early-failure/readability layer.

Related context:

- Cooperative in-process guard plan:
  [`docs/status/issue36-egress-guard-plan.md`](../status/issue36-egress-guard-plan.md)
- Capability/security boundary overview:
  [`docs/CAPABILITIES.md`](../CAPABILITIES.md#6-安全姿态分层)
- Smoke verifier:
  [`scripts/verify-egress-policy.sh`](../../scripts/verify-egress-policy.sh)

## Threat Model Layers

Layer 1 is the existing cooperative guard. It blocks loopback, RFC1918,
link-local/metadata, reserved, multicast, unspecified, and CGNAT destinations
from normal Python socket paths. It is appropriate for honest code and many
prompt-injection attempts because it produces fast, readable failures before a
network call leaves the process.

Layer 2 is the deployment boundary documented here: default-deny egress plus an
explicit public HTTPS allowlist. This is the layer that provides real
containment for malicious generated code, because the policy is enforced by the
container/network namespace/firewall rather than by monkeypatchable Python
state.

All recipes below explicitly deny:

- loopback: `127.0.0.0/8`, `::1/128`
- RFC1918 private IPv4: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
- link-local and metadata: `169.254.0.0/16`, especially
  `169.254.169.254`, and IPv6 `fe80::/10`
- CGNAT: `100.64.0.0/10`
- IPv6 unique-local: `fc00::/7`

The examples use public science HTTPS targets as an allowlist:

- `www.cbioportal.org`
- `eutils.ncbi.nlm.nih.gov`
- `www.ebi.ac.uk`
- `rest.ensembl.org`
- `api.gdc.cancer.gov`

Prefer a layer-7 explicit HTTPS proxy with SNI/host allowlisting for production
when that is available. The nftables examples below use a domain-to-IP snapshot
because nftables enforces IP addresses, not hostnames. That snapshot can break
when CDNs rotate addresses, and it can also over-allow shared CDN IPs. Treat it
as a minimal deployment recipe, not a perfect long-lived policy.

## Recipe 1: Docker `--network none`

### What

Run generated tool verification/runtime in a container with no external network
namespace routes. This is the smallest no-egress baseline.

### When To Use

Use this for offline validation, fixture tests, local artifact-only tools, or
any run where public data access is not required. If a tool must reach public
science APIs, use recipe 2 or 3 instead.

### Commands

Run from the repository root:

```bash
docker run --rm \
  --network none \
  -v "$PWD:/work:ro" \
  -w /work \
  python:3.12-bookworm \
  bash scripts/verify-egress-policy.sh
```

Use the same `--network none` boundary for the generated tool command:

```bash
docker run --rm \
  --network none \
  -v "$PWD:/work:ro" \
  -w /work \
  python:3.12-bookworm \
  python3 path/to/generated_tool.py
```

### Verification

The verifier should report that metadata/private/loopback probes are not
reachable and that the public allowlist probe is skipped because there is no
default route:

```bash
docker run --rm --network none -v "$PWD:/work:ro" -w /work \
  python:3.12-bookworm bash scripts/verify-egress-policy.sh
```

### Limits

This recipe has no public egress at all. It is the safest baseline, but it does
not support tools that must call cBioPortal, NCBI, EBI, Ensembl, or GDC. Docker
still creates a loopback interface inside the container; do not run sensitive
services inside the same container namespace.

## Recipe 2: Docker Bridge With nftables HTTPS Allowlist

### What

Create a dedicated Docker bridge, attach generated tool containers to it, and
enforce a host nftables `forward` policy that drops unsafe ranges and permits
only TCP/443 to resolved public science target IPs.

### When To Use

Use this when generated tools need public HTTPS access to a small list of known
science endpoints and Docker is available on the deployment host.

### Commands

Run these commands on the Linux Docker host. They require root for network
policy setup, but generated tool containers do not need to run as root.

```bash
set -euo pipefail

ALLOW_HOSTS=(
  www.cbioportal.org
  eutils.ncbi.nlm.nih.gov
  www.ebi.ac.uk
  rest.ensembl.org
  api.gdc.cancer.gov
)

mkdir -p .egress
: > .egress/allow-hosts
: > .egress/allow-v4
: > .egress/allow-v6

for host in "${ALLOW_HOSTS[@]}"; do
  v4="$(getent ahostsv4 "$host" | awk '$2 == "STREAM" { print $1; exit }')"
  if [ -z "$v4" ]; then
    echo "No IPv4 address resolved for $host" >&2
    exit 1
  fi
  printf '%s %s\n' "$v4" "$host" >> .egress/allow-hosts
  printf '%s\n' "$v4" >> .egress/allow-v4

  v6="$(getent ahostsv6 "$host" | awk '$2 == "STREAM" { print $1; exit }' || true)"
  if [ -n "$v6" ]; then
    printf '%s\n' "$v6" >> .egress/allow-v6
  fi
done

ALLOW_V4="$(paste -sd, .egress/allow-v4)"
ALLOW_V6="$(paste -sd, .egress/allow-v6 || true)"

docker network rm af-egress 2>/dev/null || true
docker network create \
  --driver bridge \
  --subnet 172.31.240.0/24 \
  --opt com.docker.network.bridge.name=af-egress0 \
  af-egress

sudo nft delete table inet af_egress 2>/dev/null || true
sudo nft 'add table inet af_egress'
sudo nft 'add set inet af_egress blocked_v4 { type ipv4_addr; flags interval; }'
sudo nft 'add set inet af_egress blocked_v6 { type ipv6_addr; flags interval; }'
sudo nft 'add set inet af_egress allow_https_v4 { type ipv4_addr; flags interval; }'
sudo nft 'add set inet af_egress allow_https_v6 { type ipv6_addr; flags interval; }'

sudo nft 'add element inet af_egress blocked_v4 { 0.0.0.0/8, 10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 }'
sudo nft 'add element inet af_egress blocked_v6 { ::1/128, fc00::/7, fe80::/10 }'
sudo nft "add element inet af_egress allow_https_v4 { $ALLOW_V4 }"
if [ -n "$ALLOW_V6" ]; then
  sudo nft "add element inet af_egress allow_https_v6 { $ALLOW_V6 }"
fi

sudo nft 'add chain inet af_egress forward { type filter hook forward priority -150; policy accept; }'
sudo nft add rule inet af_egress forward ct state established,related accept
sudo nft 'add rule inet af_egress forward iifname "af-egress0" ip daddr @blocked_v4 drop'
sudo nft 'add rule inet af_egress forward iifname "af-egress0" ip6 daddr @blocked_v6 drop'
sudo nft 'add rule inet af_egress forward iifname "af-egress0" tcp dport 443 ip daddr @allow_https_v4 accept'
sudo nft 'add rule inet af_egress forward iifname "af-egress0" tcp dport 443 ip6 daddr @allow_https_v6 accept'
sudo nft 'add rule inet af_egress forward iifname "af-egress0" counter drop'
```

Run a tool container with the same IP snapshot injected into `/etc/hosts`, so
the container does not need arbitrary DNS egress:

```bash
ADD_HOST_ARGS=()
while read -r ip host; do
  ADD_HOST_ARGS+=(--add-host "$host:$ip")
done < .egress/allow-hosts

CBIO_IP="$(awk '$2 == "www.cbioportal.org" { print $1; exit }' .egress/allow-hosts)"

docker run --rm \
  --network af-egress \
  "${ADD_HOST_ARGS[@]}" \
  -e AF_EGRESS_VERIFY_ASSUME_ISOLATED=1 \
  -e "AF_EGRESS_VERIFY_RESOLVE=www.cbioportal.org:443:$CBIO_IP" \
  -v "$PWD:/work:ro" \
  -w /work \
  python:3.12-bookworm \
  bash scripts/verify-egress-policy.sh
```

Use the same `--network af-egress` and `--add-host` arguments for generated
tool verification/runtime.

### Verification

Expected result:

- `169.254.169.254:80` is not reachable.
- `10.0.0.1:80` is not reachable.
- `127.0.0.1:80` is not reachable unless a service is deliberately running
  inside the same container namespace.
- `100.64.0.1:80` is not reachable.
- `https://www.cbioportal.org` is reachable through the resolved allowlist IP.

You can inspect the active policy with:

```bash
sudo nft list table inet af_egress
```

### Limits

This is an IP snapshot. CDNs can move, and shared CDN IPs may host unrelated
names. Refresh `.egress/allow-*` as part of deployment, or use an explicit
HTTPS proxy that enforces host/SNI allowlisting. Do not add broad DNS egress
unless you have separately accepted DNS exfiltration risk.

## Recipe 3: Linux network namespace + veth + nftables

### What

Create a Linux network namespace, attach it to the host with a veth pair, and
use nftables to NAT only allowlisted public HTTPS traffic while dropping unsafe
ranges.

### When To Use

Use this when Docker is unavailable or too heavy, but you can configure Linux
network namespaces and nftables on the deployment host.

### Commands

Run from the repository root on the Linux host:

```bash
set -euo pipefail

NS=af-egress
HOST_VETH=veth-af-host
NS_VETH=veth-af-ns
NS_CIDR=10.200.0.0/30
HOST_IP=10.200.0.1
NS_IP=10.200.0.2
UPLINK_IF="$(ip route show default | awk '{ print $5; exit }')"

ALLOW_HOSTS=(
  www.cbioportal.org
  eutils.ncbi.nlm.nih.gov
  www.ebi.ac.uk
  rest.ensembl.org
  api.gdc.cancer.gov
)

mkdir -p .egress
: > .egress/allow-hosts
: > .egress/allow-v4
: > .egress/allow-v6

for host in "${ALLOW_HOSTS[@]}"; do
  v4="$(getent ahostsv4 "$host" | awk '$2 == "STREAM" { print $1; exit }')"
  if [ -z "$v4" ]; then
    echo "No IPv4 address resolved for $host" >&2
    exit 1
  fi
  printf '%s %s\n' "$v4" "$host" >> .egress/allow-hosts
  printf '%s\n' "$v4" >> .egress/allow-v4

  v6="$(getent ahostsv6 "$host" | awk '$2 == "STREAM" { print $1; exit }' || true)"
  if [ -n "$v6" ]; then
    printf '%s\n' "$v6" >> .egress/allow-v6
  fi
done

ALLOW_V4="$(paste -sd, .egress/allow-v4)"
ALLOW_V6="$(paste -sd, .egress/allow-v6 || true)"

sudo ip netns delete "$NS" 2>/dev/null || true
sudo ip link delete "$HOST_VETH" 2>/dev/null || true

sudo ip netns add "$NS"
sudo ip link add "$HOST_VETH" type veth peer name "$NS_VETH"
sudo ip link set "$NS_VETH" netns "$NS"

sudo ip addr add "$HOST_IP/30" dev "$HOST_VETH"
sudo ip link set "$HOST_VETH" up
sudo ip netns exec "$NS" ip addr add "$NS_IP/30" dev "$NS_VETH"
sudo ip netns exec "$NS" ip link set lo up
sudo ip netns exec "$NS" ip link set "$NS_VETH" up
sudo ip netns exec "$NS" ip route add default via "$HOST_IP"

sudo sysctl -w net.ipv4.ip_forward=1

sudo nft delete table inet af_netns_egress 2>/dev/null || true
sudo nft delete table ip af_netns_nat 2>/dev/null || true
sudo nft 'add table inet af_netns_egress'
sudo nft 'add table ip af_netns_nat'

sudo nft 'add set inet af_netns_egress blocked_v4 { type ipv4_addr; flags interval; }'
sudo nft 'add set inet af_netns_egress blocked_v6 { type ipv6_addr; flags interval; }'
sudo nft 'add set inet af_netns_egress allow_https_v4 { type ipv4_addr; flags interval; }'
sudo nft 'add set inet af_netns_egress allow_https_v6 { type ipv6_addr; flags interval; }'

sudo nft 'add element inet af_netns_egress blocked_v4 { 0.0.0.0/8, 10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 }'
sudo nft 'add element inet af_netns_egress blocked_v6 { ::1/128, fc00::/7, fe80::/10 }'
sudo nft "add element inet af_netns_egress allow_https_v4 { $ALLOW_V4 }"
if [ -n "$ALLOW_V6" ]; then
  sudo nft "add element inet af_netns_egress allow_https_v6 { $ALLOW_V6 }"
fi

sudo nft 'add chain inet af_netns_egress forward { type filter hook forward priority -150; policy accept; }'
sudo nft add rule inet af_netns_egress forward ct state established,related accept
sudo nft add rule inet af_netns_egress forward iifname "$HOST_VETH" ip daddr @blocked_v4 drop
sudo nft add rule inet af_netns_egress forward iifname "$HOST_VETH" ip6 daddr @blocked_v6 drop
sudo nft add rule inet af_netns_egress forward iifname "$HOST_VETH" tcp dport 443 ip daddr @allow_https_v4 accept
sudo nft add rule inet af_netns_egress forward iifname "$HOST_VETH" tcp dport 443 ip6 daddr @allow_https_v6 accept
sudo nft add rule inet af_netns_egress forward iifname "$HOST_VETH" counter drop

sudo nft 'add chain ip af_netns_nat postrouting { type nat hook postrouting priority srcnat; policy accept; }'
sudo nft add rule ip af_netns_nat postrouting ip saddr "$NS_IP" oifname "$UPLINK_IF" masquerade
```

Run the verifier inside the namespace. `AF_EGRESS_VERIFY_RESOLVE` lets the
verifier connect to the resolved allowlist IP while preserving the HTTPS Host
header and SNI:

```bash
CBIO_IP="$(awk '$2 == "www.cbioportal.org" { print $1; exit }' .egress/allow-hosts)"

sudo ip netns exec af-egress env \
  AF_EGRESS_VERIFY_ASSUME_ISOLATED=1 \
  "AF_EGRESS_VERIFY_RESOLVE=www.cbioportal.org:443:$CBIO_IP" \
  bash scripts/verify-egress-policy.sh
```

Run generated tools the same way:

```bash
sudo ip netns exec af-egress env \
  "AF_EGRESS_VERIFY_RESOLVE=www.cbioportal.org:443:$CBIO_IP" \
  python3 path/to/generated_tool.py
```

For real tool execution that needs hostname resolution, prefer a namespace-local
explicit HTTPS proxy with SNI allowlisting. If you rely on IP snapshots instead,
provide controlled host resolution for the allowlisted domains only.

### Verification

Expected result is the same as recipe 2: unsafe ranges are not reachable and the
selected public HTTPS science target is reachable. Inspect with:

```bash
sudo nft list table inet af_netns_egress
sudo nft list table ip af_netns_nat
```

### Limits

This recipe changes host network state and requires root for setup/teardown. It
does not replace host hardening, user separation, seccomp/AppArmor, or VM-level
isolation when those are required. Like recipe 2, IP allowlists are vulnerable
to CDN IP drift and shared-IP over-allowing.

## Operational Notes

- Keep the Python in-process guard enabled for readable failures, but do not
  describe it as a sandbox.
- Use default-deny at the OS boundary for anti-tamper containment.
- Drop metadata, RFC1918, link-local, loopback, and CGNAT before any public
  allow rule.
- Keep public egress narrow: TCP/443 to explicit science targets, preferably
  enforced by an explicit HTTPS proxy that validates host/SNI.
- Refresh IP snapshots during deployment and treat unexpected resolver changes
  as a review event.
- The issue #36 scope is now: cooperative in-process guard delivered by PR #49,
  plus deployment-level containment recipe and smoke verifier documented here.
  After operators confirm which recipe they adopt, the issue can be narrowed or
  closed without claiming the cooperative guard is anti-tamper.
