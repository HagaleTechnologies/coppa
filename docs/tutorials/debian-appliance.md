# Install Coppa as a Debian appliance

The release workflow retains candidate `.deb` files for amd64, arm64 and armhf. Download the package for your host from the workflow's `debs-<arch>` artifact. These packages have not been published as public release assets.

```sh
sudo apt install ./coppa_0.17.0_$(dpkg --print-architecture).deb
systemctl is-active coppad.service
sudo cat /etc/coppa/setup-token
```

Open `http://127.0.0.1:8401/setup` on the host and enter the token. Select a callsign, input and output audio devices, and a PTT method. Review the settings before saving. The service checks that both audio devices open, writes `/var/lib/coppa-appliance/coppad.toml`, then starts the modem. The dashboard is at `http://127.0.0.1:8400/`; enter the same token there. Use its TUNE control only when the radio is ready.

For a headless host, keep this tunnel open on your workstation and use the local setup URL:

```sh
ssh -NT -L 127.0.0.1:8401:127.0.0.1:8401 -L 127.0.0.1:8400:127.0.0.1:8400 operator@coppa-host
```

If setup does not open, run `systemctl status coppad.service` and `journalctl -u coppad.service -n 50 --no-pager` on the host. If the journal shows that the setup listener could not bind `127.0.0.1:8401` because another service already listens there, find the other service with `ss -ltnp 'sport = :8401'`, stop it or move it to another port, then run `sudo systemctl restart coppad.service`; the wizard is served on the next start. The same applies to `127.0.0.1:8400` for the dashboard after a save, because both ports are fixed in the packaged unit. If audio devices are missing, check the sound card connection and whether the `coppa` account belongs to `audio`. Serial PTT needs device access through `dialout`; GPIO and CM108 access depends on host permissions. The package does not install rigctld or udev rules.

The root-only `/etc/coppa/setup-token` supplies the setup and dashboard credential. A valid explicit `/etc/coppa/coppad.toml` takes precedence over the managed config. To return to setup after a failed saved configuration, stop the service, back up `/var/lib/coppa-appliance/coppad.toml`, move it out of that directory, then start the service. A malformed or unreadable explicit admin config fails startup and is not replaced. Back up that file separately before changing it.

A prior manual `/etc/systemd/system/coppad.service` must be removed before installing the package. Stop the old service, back up its config and override, remove the manual unit, then install the package. The package retains the token and managed config on removal; purging removes those package-generated files. It never deletes `/etc/coppa/coppad.toml` or the service account.
