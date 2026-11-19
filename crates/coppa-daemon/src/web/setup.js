(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  let token = '';
  let inventory = {inputs: [], outputs: []};
  const show = name => {
    for (const id of ['unlock', 'station', 'review', 'done']) $(id).hidden = id !== name;
    $('message').textContent = '';
  };
  const message = value => { $('message').textContent = value; };
  const request = async (path, options = {}) => {
    const response = await fetch(path, { ...options, headers: {Authorization: `Bearer ${token}`, ...(options.headers || {})}, cache: 'no-store' });
    const data = await response.json();
    if (!response.ok) throw data;
    return data;
  };
  const options = (id, devices) => {
    const old = $(id).value;
    $(id).replaceChildren();
    const placeholder = new Option(`Choose an ${id.startsWith('input') ? 'input' : 'output'}`, '');
    $(id).add(placeholder);
    for (const d of devices) $(id).add(new Option(d.name, d.id));
    $(id).value = devices.some(d => d.id === old) ? old : '';
  };
  const refresh = async () => {
    message('Looking for audio devices…');
    try {
      inventory = await request('/api/devices');
      options('input_device_id', inventory.inputs);
      options('output_device_id', inventory.outputs);
      message(inventory.inputs.length && inventory.outputs.length ? '' : 'Connect audio input and output devices, then refresh.');
    } catch { message('Could not list audio devices. Refresh to try again.'); }
  };
  const pttFields = {
    none: '', vox: '',
    rigctld: '<label>rigctld address<input id="address" value="127.0.0.1:4532"></label>',
    serial: '<label>Serial device path<input id="path"></label><label>Control line<select id="line"><option value="rts">RTS</option><option value="dtr">DTR</option></select></label>',
    gpio: '<label>GPIO chip<input id="chip"></label><label>GPIO line<input id="line" type="number" min="0"></label><label><input id="active_low" type="checkbox">Active low</label>',
    cm108: '<label>hidraw device path<input id="path"></label><label>GPIO pin (optional)<input id="pin" type="number" min="0"></label><label><input id="active_low" type="checkbox">Active low</label>'
  };
  $('method').onchange = () => { $('ptt-fields').innerHTML = pttFields[$('method').value]; };
  $('method').onchange();
  const draft = () => {
    const method = $('method').value;
    const ptt = {method};
    for (const field of ['address', 'path', 'chip', 'line', 'pin']) {
      const input = $(field);
      if (input && input.value !== '') ptt[field] = ['pin', 'line'].includes(field) && method !== 'serial' ? Number(input.value) : input.value;
    }
    if ($('active_low')) ptt.active_low = $('active_low').checked;
    return {callsign: $('callsign').value, grid: $('grid').value,
      input_device_id: $('input_device_id').value, output_device_id: $('output_device_id').value,
      ptt, beacon_interval_secs: Number($('beacon_interval_secs').value)};
  };
  $('unlock-next').onclick = async () => {
    token = $('token').value.trim();
    if (!token) { message('Enter the access token.'); return; }
    message('Checking access token…');
    try { await request('/api/config'); show('station'); await refresh(); }
    catch (e) { message(e && e.error === 'unauthorized' ? 'Access token rejected. Check the token and try again.' : 'Could not reach setup. Check the service or SSH tunnel and try again.'); }
  };
  $('refresh').onclick = refresh;
  $('station-back').onclick = () => show('unlock');
  $('review-next').onclick = () => {
    const data = draft();
    if (!data.callsign || !data.input_device_id || !data.output_device_id) { message('Enter a valid callsign and select an audio input and output.'); return; }
    const pttDetails = Object.entries(data.ptt).filter(([key]) => key !== 'method').map(([key, value]) => `${key}: ${value}`).join('\n');
    $('summary').textContent = `Callsign: ${data.callsign}\nGrid locator: ${data.grid || 'Not set'}\nAudio input: ${inventory.inputs.find(d => d.id === data.input_device_id)?.name || data.input_device_id}\nAudio output: ${inventory.outputs.find(d => d.id === data.output_device_id)?.name || data.output_device_id}\nPTT method: ${data.ptt.method}\n${pttDetails}\nBeacon interval: ${data.beacon_interval_secs ? `${data.beacon_interval_secs} seconds (automatic transmission enabled)` : 'Off'}`;
    show('review');
  };
  $('review-back').onclick = () => show('station');
  $('save').onclick = async () => {
    message('Checking audio and saving settings…');
    try {
      const result = await request('/api/config', {method: 'PUT', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(draft())});
      $('dashboard').href = result.dashboard_url;
      show('done');
    } catch (e) {
      message(e?.error === 'audio_unavailable' ? 'Could not open the selected audio devices. Check the connection and permissions, then try again.' :
        e?.error === 'already_configured' ? 'Setup is already complete. Open dashboard.' :
        e?.error === 'invalid_settings' ? 'Check the station, PTT settings and beacon interval.' :
        'Could not save settings. Check the service log and try again.');
    }
  };
})();
