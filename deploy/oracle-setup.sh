#!/usr/bin/env bash
# One-paste setup of the LMU Race Engineering relay on a fresh Oracle Cloud "Always Free" Ubuntu VM
# with a free DuckDNS name.
#
#   curl -fsSL https://raw.githubusercontent.com/UnknownGoat11/Race-Engineering-Software/dev/deploy/oracle-setup.sh -o setup.sh
#   sudo bash setup.sh
#
# It asks for the DuckDNS subdomain, the DuckDNS token and an e-mail address (Let's Encrypt expiry notices),
# then: installs Docker and certbot, opens ports 80/443 in the VM firewall, points the DuckDNS name at this VM
# (and keeps it updated), gets a Let's Encrypt certificate (renewed automatically), builds and starts the relay
# with PostgreSQL, and enables in-app account creation. Safe to run again: finished steps are skipped.
#
# Before running it, the Oracle network must allow TCP 80 and 443 in (VCN security list ingress rules).
set -euo pipefail

REPO_URL="https://github.com/UnknownGoat11/Race-Engineering-Software.git"
REPO_BRANCH="dev"
INSTALL_DIR="/opt/lmu-relay"
TLS_DIR="/etc/lmu-relay/tls"
DUCKDNS_CONF="/root/.duckdns"

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || fail "run it with sudo: sudo bash setup.sh"
[ -r /etc/os-release ] && . /etc/os-release
[ "${ID:-}" = "ubuntu" ] || fail "this script expects Ubuntu (choose the Canonical Ubuntu image on Oracle)"

# ------------------------------------------------------------------------------------------------- questions
if [ -r "$DUCKDNS_CONF" ]; then
  # shellcheck disable=SC1090
  . "$DUCKDNS_CONF"
fi
if [ -z "${DUCKDNS_SUBDOMAIN:-}" ]; then
  read -r -p "DuckDNS subdomain (only the first part, e.g. lmurelay for lmurelay.duckdns.org): " DUCKDNS_SUBDOMAIN
fi
DUCKDNS_SUBDOMAIN="${DUCKDNS_SUBDOMAIN%.duckdns.org}"
[[ "$DUCKDNS_SUBDOMAIN" =~ ^[a-z0-9-]+$ ]] || fail "the subdomain may only contain a-z, 0-9 and -"
if [ -z "${DUCKDNS_TOKEN:-}" ]; then
  read -r -p "DuckDNS token (shown at the top of duckdns.org after you log in): " DUCKDNS_TOKEN
fi
[[ "$DUCKDNS_TOKEN" =~ ^[0-9a-f-]{36}$ ]] || fail "that does not look like a DuckDNS token (36 characters like 1a2b3c4d-....)"
if [ -z "${LE_EMAIL:-}" ]; then
  read -r -p "Your e-mail (only for certificate expiry warnings): " LE_EMAIL
fi
[[ "$LE_EMAIL" == *@*.* ]] || fail "please enter a valid e-mail address"
HOST="${DUCKDNS_SUBDOMAIN}.duckdns.org"

umask 077
cat > "$DUCKDNS_CONF" <<EOF
DUCKDNS_SUBDOMAIN=$DUCKDNS_SUBDOMAIN
DUCKDNS_TOKEN=$DUCKDNS_TOKEN
LE_EMAIL=$LE_EMAIL
EOF
umask 022

# ------------------------------------------------------------------------------------------------- packages
step "Installing system packages (a few minutes)"
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
echo iptables-persistent iptables-persistent/autosave_v4 boolean true | debconf-set-selections
echo iptables-persistent iptables-persistent/autosave_v6 boolean true | debconf-set-selections
apt-get install -y -q ca-certificates curl git openssl certbot cron iptables-persistent

if ! command -v docker >/dev/null 2>&1; then
  step "Installing Docker"
  curl -fsSL https://get.docker.com | sh
fi
systemctl enable --now docker >/dev/null
docker compose version >/dev/null 2>&1 || fail "docker compose is missing after the Docker install"

# ------------------------------------------------------------------------------------------------- firewall
step "Opening ports 80 and 443 in the VM firewall"
for port in 80 443; do
  if ! iptables -C INPUT -p tcp -m state --state NEW --dport "$port" -j ACCEPT 2>/dev/null; then
    iptables -I INPUT 1 -p tcp -m state --state NEW --dport "$port" -j ACCEPT
  fi
done
netfilter-persistent save >/dev/null

# ------------------------------------------------------------------------------------------------- DuckDNS
step "Pointing $HOST at this server"
cat > /usr/local/bin/duckdns-update <<'EOF'
#!/bin/sh
. /root/.duckdns
curl -fsS --max-time 20 "https://www.duckdns.org/update?domains=${DUCKDNS_SUBDOMAIN}&token=${DUCKDNS_TOKEN}&ip="
EOF
chmod 0700 /usr/local/bin/duckdns-update
answer="$(/usr/local/bin/duckdns-update || true)"
[ "$answer" = "OK" ] || fail "DuckDNS refused the update (answer: '${answer}'). Check the subdomain and the token."
echo "*/5 * * * * root /usr/local/bin/duckdns-update >/dev/null 2>&1" > /etc/cron.d/duckdns
chmod 0644 /etc/cron.d/duckdns
systemctl enable --now cron >/dev/null

public_ip="$(curl -4 -fsS --max-time 10 https://ifconfig.me || true)"
echo "This server's public IP: ${public_ip:-unknown}. Waiting for $HOST to point here..."
for _ in $(seq 1 30); do
  resolved="$(getent ahostsv4 "$HOST" | awk 'NR==1{print $1}')"
  [ -n "$resolved" ] && [ "$resolved" = "$public_ip" ] && break
  sleep 5
done
echo "$HOST -> ${resolved:-not resolved yet}"

# ------------------------------------------------------------------------------------------------- source
step "Downloading the relay source"
if [ -d "$INSTALL_DIR/.git" ]; then
  git -C "$INSTALL_DIR" fetch -q --depth 1 origin "$REPO_BRANCH"
  git -C "$INSTALL_DIR" reset -q --hard "origin/$REPO_BRANCH"
else
  git clone -q --depth 1 --branch "$REPO_BRANCH" "$REPO_URL" "$INSTALL_DIR"
fi
cd "$INSTALL_DIR/deploy"

# ------------------------------------------------------------------------------------------------- certificate
step "Getting the TLS certificate from Let's Encrypt"
mkdir -p "$TLS_DIR"
HOOK="install -m 0640 -o root -g 65532 \"\$RENEWED_LINEAGE/fullchain.pem\" $TLS_DIR/fullchain.pem && install -m 0640 -o root -g 65532 \"\$RENEWED_LINEAGE/privkey.pem\" $TLS_DIR/privkey.pem"
if [ ! -s "$TLS_DIR/fullchain.pem" ]; then
  certbot certonly --standalone --non-interactive --agree-tos -m "$LE_EMAIL" -d "$HOST" --deploy-hook "$HOOK" \
    || fail "Let's Encrypt could not reach this server on port 80. Check the Oracle ingress rules for TCP 80 and 443."
fi
[ -s "$TLS_DIR/fullchain.pem" ] || fail "the certificate was not installed into $TLS_DIR"

# ------------------------------------------------------------------------------------------------- config
step "Writing the relay configuration"
umask 077
if [ ! -s .env ]; then
  printf 'POSTGRES_PASSWORD=%s\n' "$(openssl rand -hex 24)" > .env
fi
if [ ! -s relay.env ]; then
  cp relay.env.example relay.env
  sed -i "s/^ADMIN_TOKEN=.*/ADMIN_TOKEN=$(openssl rand -hex 32)/" relay.env
  # Users create their accounts from the app's "Create account" tab (per-IP rate limits apply).
  sed -i "s/^RELAY_OPEN_REGISTRATION=.*/RELAY_OPEN_REGISTRATION=true/" relay.env
fi
umask 022

# ------------------------------------------------------------------------------------------------- build
step "Building the relay (first time about 10-20 minutes, please wait)"
docker compose build

if [ ! -s secrets/jwt.key ]; then
  step "Creating the signing key"
  mkdir -p secrets
  docker run --rm -v "$PWD/secrets:/out" --user 0:0 lmu-relay:latest gen-jwt-key --out /out/jwt.key
  chown 65532:65532 secrets/jwt.key
  chmod 0400 secrets/jwt.key
fi

# ------------------------------------------------------------------------------------------------- start
step "Starting the relay"
docker compose up -d
container="$(docker compose ps -q relay)"
status="starting"
for _ in $(seq 1 40); do
  status="$(docker inspect --format '{{.State.Health.Status}}' "$container" 2>/dev/null || echo unknown)"
  [ "$status" = "healthy" ] && break
  sleep 3
done
[ "$status" = "healthy" ] || { docker compose logs --tail 50 relay; fail "the relay did not become healthy (status: $status)"; }

printf '\n\033[1;32mDone. The relay is running.\033[0m\n'
echo "  Relay address:  $HOST"
echo "  Port:           443"
echo "  Accounts:       created by users in the app (Create account tab)"
echo "  Logs:           cd $INSTALL_DIR/deploy && sudo docker compose logs -f relay"
echo "  Update later:   sudo bash ~/setup.sh   (same command as the first time)"
echo
echo "Send this address to Claude so it can be built into the app: $HOST"
