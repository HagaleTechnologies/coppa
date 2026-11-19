#!/bin/sh
# Run only on a disposable systemd release runner.
set -eu
package=$1
sudo apt-get install -y "./$package"
test "$(systemctl is-active coppad.service)" = active
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if curl --fail --silent http://127.0.0.1:8401/setup >/dev/null 2>&1; then break; fi
  sleep 1
done
curl --fail --silent --show-error http://127.0.0.1:8401/setup >/dev/null
token=$(sudo cat /etc/coppa/setup-token)
body='{"callsign":"K1ABC","grid":"FN20","input_device_id":"alsa:null","output_device_id":"alsa:null","ptt":{"method":"none"},"beacon_interval_secs":0}'
curl --fail --silent --show-error --output /dev/null \
  -X PUT -H "Host: 127.0.0.1:8401" -H 'Origin: http://127.0.0.1:8401' \
  -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
  --data "$body" http://127.0.0.1:8401/api/config
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if curl --fail --silent --show-error http://127.0.0.1:8400/health >/dev/null 2>&1; then break; fi
  sleep 1
done
curl --fail --silent --show-error http://127.0.0.1:8400/health >/dev/null
sudo systemctl restart coppad.service
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if curl --fail --silent http://127.0.0.1:8400/health >/dev/null 2>&1; then break; fi
  sleep 1
done
curl --fail --silent --show-error http://127.0.0.1:8400/health >/dev/null
sudo apt-get remove -y coppa
test -f /etc/coppa/setup-token
test -f /var/lib/coppa-appliance/coppad.toml
sudo apt-get purge -y coppa
test ! -e /etc/coppa/setup-token
test ! -e /var/lib/coppa-appliance/coppad.toml
