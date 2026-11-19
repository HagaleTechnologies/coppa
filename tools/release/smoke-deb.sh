#!/bin/sh
# Run only on a disposable systemd release runner.
set -eu
package=$1

# On any failure, print the unit state and journal so the job log shows why.
report() {
  status=$?
  if [ "$status" -ne 0 ]; then
    sudo systemctl status --no-pager coppad.service || true
    sudo journalctl -u coppad.service -n 100 --no-pager || true
  fi
  exit "$status"
}
trap report EXIT

wait_for() {
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    if curl --fail --silent "$1" >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  curl --fail --silent --show-error "$1" >/dev/null
}

sudo apt-get install -y "./$package"
# Type=exec reports active as soon as coppad starts, so wait for the setup
# page first: a daemon that exits early (for example on a permission error)
# fails here and again on the is-active check.
wait_for http://127.0.0.1:8401/setup
test "$(systemctl is-active coppad.service)" = active
# The service account runs coppad, so it must be able to search the
# root-owned admin directory (it cannot read the token inside).
sudo -u coppa test -x /etc/coppa
sudo -u coppa test ! -r /etc/coppa/setup-token
token=$(sudo cat /etc/coppa/setup-token)
body='{"callsign":"K1ABC","grid":"FN20","input_device_id":"alsa:null","output_device_id":"alsa:null","ptt":{"method":"none"},"beacon_interval_secs":0}'
curl --fail --silent --show-error --output /dev/null \
  -X PUT -H "Host: 127.0.0.1:8401" -H 'Origin: http://127.0.0.1:8401' \
  -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
  --data "$body" http://127.0.0.1:8401/api/config
wait_for http://127.0.0.1:8400/health
sudo systemctl restart coppad.service
wait_for http://127.0.0.1:8400/health
sudo apt-get remove -y coppa
# Both paths are root- or coppa-only, so check them with sudo.
sudo test -f /etc/coppa/setup-token
sudo test -f /var/lib/coppa-appliance/coppad.toml
sudo apt-get purge -y coppa
sudo test ! -e /etc/coppa/setup-token
sudo test ! -e /var/lib/coppa-appliance/coppad.toml
