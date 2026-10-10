#!/usr/bin/env bash
set -euo pipefail
# Isolated loopback fixture. No user accounts, settings or external TURN service.
fixture=$(mktemp -d)
trap 'kill "${turn_pid:-}" 2>/dev/null || true; rm -rf "$fixture"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj '/CN=ParsecWebTurn CI CA' -keyout "$fixture/ca.key" -out "$fixture/ca.crt" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj '/CN=localhost' -keyout "$fixture/server.key" -out "$fixture/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n' > "$fixture/extensions"
openssl x509 -req -in "$fixture/server.csr" -CA "$fixture/ca.crt" -CAkey "$fixture/ca.key" -CAcreateserial -days 1 -extfile "$fixture/extensions" -out "$fixture/server.crt" 2>/dev/null
sudo cp "$fixture/ca.crt" /usr/local/share/ca-certificates/parsec-ci-turn.crt
sudo update-ca-certificates >/dev/null
turnserver --listening-ip=127.0.0.1 --relay-ip=127.0.0.1 --listening-port=43478 --tls-listening-port=43549 \
  --min-port=49160 --max-port=49200 --realm=parsec-ci --user=fixture:fixture-password --lt-cred-mech \
  --cert="$fixture/server.crt" --pkey="$fixture/server.key" --allow-loopback-peers --no-cli --no-dtls --no-multicast-peers \
  --userdb="$fixture/turn.db" --pidfile="$fixture/turn.pid" --log-file=stdout > "$fixture/turn.log" 2>&1 &
turn_pid=$!
sleep 2
if ! cargo test --locked --manifest-path tests/turn-integration/Cargo.toml --target x86_64-unknown-linux-gnu -- --nocapture; then
  cat "$fixture/turn.log"
  exit 1
fi
